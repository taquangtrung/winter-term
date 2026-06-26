//! The native application: winit event loop, GPU renderer, and PTY panes wired
//! together. This is the `Winter` binary's core runtime — keyboard input drives
//! the PTY, PTY output drives the cell grid, and the grid is rendered to the
//! GPU surface every frame.
//!
//! Submodules split the `App`'s responsibilities by concern:
//! - [`init`] — GPU/window bootstrap on `resumed`.
//! - [`actions`] — keyboard action dispatch.
//! - [`render`] — frame composition and WebView tile management.
//! - [`blocks`] — block fold / yank / focus operations.
//! - [`navigation`] — vim-style cursor motions, search, quick-select.
//! - [`pointer`] — mouse hit-testing, selection, clipboard, PTY mouse forwarding.

pub mod actions;
mod blocks;
mod init;
mod navigation;
mod pointer;
mod prompt_edit;
mod tabbar;
use prompt_edit::PromptShadow;
mod render;
mod session_restore;

use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

use crate::config::{
    expand_window_title_template, Config, StatusBarConfig, TitleBarStyle, WindowTitleVars,
    DEFAULT_WINDOW_TITLE_TEMPLATE,
};
use crate::control::{self, ControlMessage};
use crate::model::input::{
    self, Action, EditBinding, KeyCode, PendingPrefix, VisualKind, WindowKeymap,
};
use crate::model::layout::{Direction, FocusDir, PaneId, Rect, Tab};
use crate::model::mode::Mode;
use crate::model::palette::{Palette, PaletteMode};
use crate::model::settings_page::{ChoiceOption, SettingsField, SettingsPage};
use crate::session::Session;
use crate::terminal::pane::Pane;
use crate::terminal::webview::WebViewManager;
use winter_render::renderer::{GpuRenderer, PaneRect};
use winter_render::{
    ControlsSide, CursorShape, MenuStyle, NoticeKind, StatusBar, StatusNotice, StatusSearch,
    TabbarHit, Theme,
};

// ========================================================================
// Constants
// ========================================================================

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
/// Period of one cursor blink phase (on or off). Two phases = one full blink cycle.
const CURSOR_BLINK_PERIOD: Duration = Duration::from_millis(530);
const SPLIT_RATIO: f32 = 0.5;
/// Cell rows reserved at the bottom of the surface for the status bar.
pub(crate) const STATUS_BAR_ROWS: usize = 1;
/// How long a transient status-bar notice stays on screen before it expires
/// and the bar returns to showing the pane title.
const NOTICE_DURATION: Duration = Duration::from_secs(3);
/// Pixel margin from the top/bottom of the content viewport within which a
/// held-button selection drag auto-scrolls the pane's scrollback.
const AUTO_SCROLL_EDGE_MARGIN: f32 = 24.0;
/// How many history lines one auto-scroll step moves at full edge depth (the
/// pointer at or past the viewport's own edge). Barely inside the margin still
/// crawls at one line per step — the speed scales linearly in between, so
/// holding deeper into the edge scrolls proportionally faster.
const AUTO_SCROLL_MAX_LINES_PER_TICK: usize = 4;
/// Minimum time between auto-scroll steps while the pointer sits in the edge
/// margin, so history advances at a readable pace rather than once per
/// `about_to_wait` tick.
const AUTO_SCROLL_INTERVAL: Duration = Duration::from_millis(50);
/// How soon a second bare Escape on the same pane must follow the first
/// PTY-forwarded one to count as a double tap and switch to Normal mode
/// instead - see [`App::last_alt_screen_escape`].
const ALT_SCREEN_ESCAPE_DOUBLE_TAP: Duration = Duration::from_millis(400);

/// Bounds and step for the settings page's font-size and opacity rows. The
/// opacity range matches the clamp in [`App::apply_setting`].
/// Pixel width of the scrollbar hit region at the right edge of each pane.
const SCROLLBAR_CLICK_WIDTH: f32 = 8.0;
/// Pixel margin from the window's outer edge within which a press starts an
/// OS-driven `drag_resize_window`. Only applies to the Modern title bar: with
/// OS decorations off, nothing else offers a resize border.
const WINDOW_RESIZE_BORDER_PX: f32 = 6.0;
const MIN_FONT_SIZE: f32 = 6.0;
const MAX_FONT_SIZE: f32 = 72.0;
const FONT_SIZE_STEP: f32 = 1.0;
const MIN_OPACITY: f32 = 0.1;
const MAX_OPACITY: f32 = 1.0;
const OPACITY_STEP: f32 = 0.05;
const MIN_SCROLLBACK: f32 = 100.0;
const MAX_SCROLLBACK: f32 = 100_000.0;
const SCROLLBACK_STEP: f32 = 1_000.0;

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

/// The pane band inside `available_px` (the pixels between the top chrome and the
/// one-row status bar): how many whole cell rows fit, and the padding that
/// centers the leftover sub-row slack above them.
///
/// Shared by [`App::viewport_rect`] and `render_frame` so the drawn geometry,
/// pointer hit-testing and the PTY's row count can't drift. Deriving the rows
/// from the pixels actually left — rather than subtracting chrome rows from the
/// window's row count — is what keeps the panes clear of the status bar when the
/// Modern tabbar's extra pixels eat into the slack.
pub(crate) fn content_band(available_px: f32, ch: f32) -> (usize, f32) {
    if ch <= 0.0 {
        return (1, 0.0);
    }
    let rows = ((available_px / ch).floor() as usize).max(1);
    let pad = ((available_px - rows as f32 * ch) / 2.0).floor().max(0.0);
    (rows, pad)
}

/// The outer-edge/corner resize direction for a point at `(x, y)` in a `w`x`h`
/// window, or `None` once it's more than `border` from every edge. Corners
/// win over edges, so a top-left pixel resolves to `NorthWest`, not `North`.
pub(crate) fn edge_resize_direction_at(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    border: f32,
) -> Option<ResizeDirection> {
    let west = x <= border;
    let east = x >= w - border;
    let north = y <= border;
    let south = y >= h - border;
    if north && west {
        Some(ResizeDirection::NorthWest)
    } else if north && east {
        Some(ResizeDirection::NorthEast)
    } else if south && west {
        Some(ResizeDirection::SouthWest)
    } else if south && east {
        Some(ResizeDirection::SouthEast)
    } else if west {
        Some(ResizeDirection::West)
    } else if east {
        Some(ResizeDirection::East)
    } else if north {
        Some(ResizeDirection::North)
    } else if south {
        Some(ResizeDirection::South)
    } else {
        None
    }
}

/// The nearest window height with zero leftover slack below a whole number
/// of cell rows, given `top_h`/`status_h` pixels of fixed chrome and `ch`-tall
/// rows in between.
pub(crate) fn snap_height_to_rows(h: f32, top_h: f32, status_h: f32, ch: f32) -> f32 {
    let rows = ((h - top_h - status_h) / ch).round().max(1.0);
    // Ceiling, not nearest: `content_band` floors back down to a row count at
    // render time, so if `top_h`/`status_h` aren't whole pixels (e.g. the
    // Modern tabbar), rounding to nearest can land a fraction of a pixel
    // under the exact height and lose the whole row it was aiming to keep.
    (top_h + rows * ch + status_h).ceil()
}

/// The nearest window width with zero leftover slack past a whole number of
/// cell columns, given `h_pad` pixels of fixed horizontal chrome (both edges'
/// [`winter_render::PANE_H_PAD`] combined) and `cw`-wide columns in between.
pub(crate) fn snap_width_to_cols(w: f32, h_pad: f32, cw: f32) -> f32 {
    let cols = ((w - h_pad) / cw).round().max(1.0);
    // Ceiling for the same reason as `snap_height_to_rows`: the renderer
    // floors back down to a column count, so rounding to nearest can land
    // under the exact width and lose the whole column it was aiming to keep.
    (cols * cw + h_pad).ceil()
}

fn status_bar(
    mode: Mode,
    theme: &Theme,
    search: Option<StatusSearch>,
    notice: Option<StatusNotice>,
    config: &StatusBarConfig,
) -> StatusBar {
    let icons = &config.icons;
    let (mode_name, accent) = match mode {
        Mode::Insert => ("Insert", theme.ansi[12]),
        Mode::Normal => ("Normal", theme.ansi[4]),
        Mode::Visual => ("Visual", theme.ansi[6]),
        Mode::BlockFocus => ("Block", theme.ansi[5]),
    };
    // Visual shares the Normal icon: it is a navigation sub-mode, not a separate
    // configurable glyph.
    let mode_icon = match mode {
        Mode::Insert => &icons.insert,
        Mode::Normal | Mode::Visual => &icons.normal,
        Mode::BlockFocus => &icons.block,
    };
    let mode_label = if !config.show_mode {
        String::new()
    } else if mode_icon.is_empty() {
        mode_name.to_string()
    } else {
        format!("{} {}", mode_icon, mode_name)
    };
    StatusBar {
        accent,
        mode: mode_label,
        search,
        notice,
    }
}

/// The theme dropdown choices for the settings page: the three built-ins, then
/// each `themes/*.kdl` file by name. Mirrors the values [`ThemeSetting`] accepts.
fn settings_theme_options() -> Vec<ChoiceOption> {
    let mut options = vec![
        ChoiceOption {
            label: "Dark".into(),
            value: "dark".into(),
        },
        ChoiceOption {
            label: "Light".into(),
            value: "light".into(),
        },
        ChoiceOption {
            label: "Auto (follow system)".into(),
            value: "auto".into(),
        },
    ];
    for name in crate::config::available_themes() {
        options.push(ChoiceOption {
            label: name.clone(),
            value: name,
        });
    }
    options
}

/// Split a MuxNew palette query into `(session name, command line)`: the
/// first whitespace-delimited word names the session, the rest is the
/// command it runs (the default shell when absent). `None` when there is
/// no usable name — missing, placeholder-prefixed (those entries start
/// with `(` by convention), or carrying control characters.
fn parse_mux_spawn_query(query: &str) -> Option<(String, Option<String>)> {
    let mut words = query.split_whitespace();
    let name = words.next()?.to_string();
    if name.starts_with('(') || name.chars().any(char::is_control) {
        return None;
    }
    let command = words.collect::<Vec<_>>().join(" ");
    Some((name, (!command.is_empty()).then_some(command)))
}

/// Split a MuxAttachRemote palette query into `(host, session name)`: the
/// first word names the ssh host, the rest (if any) names the session.
/// `None` when there is no usable host.
fn parse_mux_attach_query(query: &str) -> Option<(String, Option<String>)> {
    let mut words = query.split_whitespace();
    let host = words.next()?.to_string();
    if host.starts_with('(') || host.chars().any(char::is_control) {
        return None;
    }
    let session = words.collect::<Vec<_>>().join(" ");
    Some((host, (!session.is_empty()).then_some(session)))
}

/// Recover a character from `physical`'s key position when the logical key
/// couldn't resolve one. On some keyboard layouts, Windows' `ToUnicode`
/// returns no character at all for certain Ctrl+Alt digit/letter combos
/// (the same class of layout quirk that defines `AltGr`), so winit reports
/// `Key::Unidentified` even though a physical digit or letter was pressed.
/// Covers only the keys chord specs actually use.
fn physical_key_fallback(physical: &PhysicalKey) -> Option<char> {
    use winit::keyboard::KeyCode as Physical;
    let PhysicalKey::Code(code) = physical else {
        return None;
    };
    Some(match code {
        Physical::Digit0 => '0',
        Physical::Digit1 => '1',
        Physical::Digit2 => '2',
        Physical::Digit3 => '3',
        Physical::Digit4 => '4',
        Physical::Digit5 => '5',
        Physical::Digit6 => '6',
        Physical::Digit7 => '7',
        Physical::Digit8 => '8',
        Physical::Digit9 => '9',
        Physical::KeyA => 'a',
        Physical::KeyB => 'b',
        Physical::KeyC => 'c',
        Physical::KeyD => 'd',
        Physical::KeyE => 'e',
        Physical::KeyF => 'f',
        Physical::KeyG => 'g',
        Physical::KeyH => 'h',
        Physical::KeyI => 'i',
        Physical::KeyJ => 'j',
        Physical::KeyK => 'k',
        Physical::KeyL => 'l',
        Physical::KeyM => 'm',
        Physical::KeyN => 'n',
        Physical::KeyO => 'o',
        Physical::KeyP => 'p',
        Physical::KeyQ => 'q',
        Physical::KeyR => 'r',
        Physical::KeyS => 's',
        Physical::KeyT => 't',
        Physical::KeyU => 'u',
        Physical::KeyV => 'v',
        Physical::KeyW => 'w',
        Physical::KeyX => 'x',
        Physical::KeyY => 'y',
        Physical::KeyZ => 'z',
        _ => return None,
    })
}

fn winit_key_to_code(key: &Key, physical: &PhysicalKey) -> KeyCode {
    match key.as_ref() {
        Key::Character(c) => KeyCode::Char(c.chars().next().unwrap_or('\0')),
        Key::Named(NamedKey::Enter) => KeyCode::Enter,
        Key::Named(NamedKey::Backspace) => KeyCode::Backspace,
        Key::Named(NamedKey::Tab) => KeyCode::Tab,
        Key::Named(NamedKey::Escape) => KeyCode::Escape,
        Key::Named(NamedKey::ArrowUp) => KeyCode::Up,
        Key::Named(NamedKey::ArrowDown) => KeyCode::Down,
        Key::Named(NamedKey::ArrowLeft) => KeyCode::Left,
        Key::Named(NamedKey::ArrowRight) => KeyCode::Right,
        Key::Named(NamedKey::Space) => KeyCode::Space,
        Key::Named(NamedKey::Home) => KeyCode::Home,
        Key::Named(NamedKey::End) => KeyCode::End,
        Key::Named(NamedKey::PageUp) => KeyCode::PageUp,
        Key::Named(NamedKey::PageDown) => KeyCode::PageDown,
        Key::Named(NamedKey::Insert) => KeyCode::Insert,
        Key::Named(NamedKey::Delete) => KeyCode::Delete,
        Key::Named(NamedKey::F1) => KeyCode::F(1),
        Key::Named(NamedKey::F2) => KeyCode::F(2),
        Key::Named(NamedKey::F3) => KeyCode::F(3),
        Key::Named(NamedKey::F4) => KeyCode::F(4),
        Key::Named(NamedKey::F5) => KeyCode::F(5),
        Key::Named(NamedKey::F6) => KeyCode::F(6),
        Key::Named(NamedKey::F7) => KeyCode::F(7),
        Key::Named(NamedKey::F8) => KeyCode::F(8),
        Key::Named(NamedKey::F9) => KeyCode::F(9),
        Key::Named(NamedKey::F10) => KeyCode::F(10),
        Key::Named(NamedKey::F11) => KeyCode::F(11),
        Key::Named(NamedKey::F12) => KeyCode::F(12),
        _ => physical_key_fallback(physical).map_or(KeyCode::Char('\0'), KeyCode::Char),
    }
}

/// Whether `action` actually put bytes on the pty. A key winit cannot identify
/// (e.g. a synthesized focus-in event for an already-held modifier) resolves to
/// an empty `SendBytes`, which must not be folded into the prompt shadow as a
/// real keystroke: doing so would wrongly mark an untouched, truly empty line
/// as non-empty and desync undo/redo tracking until the next `Enter`.
fn forwarded_to_pty(action: &Action) -> bool {
    matches!(action, Action::SendBytes(bytes) if !bytes.is_empty())
}

impl App {
    /// Bookkeeping for one key forwarded to the PTY while the pane is in
    /// Insert mode: keep the prompt shadow in step so `Ctrl-/`/`Ctrl-\\` can
    /// replay edits, and accumulate the exact bytes into the Insert session's
    /// `.`-repeat run (finalized when the pane leaves Insert — see
    /// [`App::track_change`]). Extracted from the keyboard handler so tests
    /// can drive the same path the event loop does.
    pub(crate) fn record_insert_key(
        &mut self,
        focused: PaneId,
        key: &input::Key,
        action: &Action,
        at_prompt: bool,
    ) {
        self.prompt_shadows
            .entry(focused)
            .or_default()
            .apply_insert_key(key, at_prompt);
        if let Action::SendBytes(bytes) = action {
            self.insert_sessions
                .entry(focused)
                .or_default()
                .run
                .extend_from_slice(bytes);
        }
        // Tracks whether the pane may now be mid the shell's own
        // tab-completion, so a following bare Escape can cancel it -
        // see `escape_forwarded_to_pty`.
        if key.code == input::KeyCode::Tab {
            self.pending_tab_completion.insert(focused);
        } else {
            self.pending_tab_completion.remove(&focused);
        }
    }
}

/// Whether a bare Escape (no modifiers) in Insert mode should be written
/// straight to the PTY as `0x1b` instead of switching the pane to Normal
/// mode: either a foreground process already owns the pane (a full-screen
/// app), or the pane is mid the shell's own tab-completion (e.g. zsh's
/// menu-select) - shell-internal state this app has no other way to
/// observe, so it needs the real ESC byte to cancel itself rather than
/// having Normal mode switch in underneath it.
fn escape_forwarded_to_pty(has_foreground_process: bool, pending_tab_completion: bool) -> bool {
    has_foreground_process || pending_tab_completion
}

/// Whether `now` is a second bare Escape completing a double tap against
/// `prev`, the pane and instant of the last one forwarded to a full-screen
/// app - see [`App::last_alt_screen_escape`]. `prev` must come from the same
/// pane and land within [`ALT_SCREEN_ESCAPE_DOUBLE_TAP`] of `now`.
fn is_alt_screen_escape_double_tap(
    prev: Option<(PaneId, Instant)>,
    pane: PaneId,
    now: Instant,
) -> bool {
    prev.is_some_and(|(prev_pane, at)| {
        prev_pane == pane && now.duration_since(at) < ALT_SCREEN_ESCAPE_DOUBLE_TAP
    })
}

/// Whether a plain Escape should clear a mouse-drag selection instead of
/// falling through to its usual mode-switch/PTY-forward handling. Visual
/// mode is excluded: there Escape already clears the selection via the
/// Normal-mode switch, together with the mode's own anchor state.
fn escape_clears_selection(key: &input::Key, mode: Mode, has_selection: bool) -> bool {
    key.code == KeyCode::Escape
        && !key.ctrl
        && !key.alt
        && !key.shift
        && mode != Mode::Visual
        && has_selection
}

/// Whether a `KeyboardInput` event should be dropped as a Windows-only race:
/// Win32 can dispatch a queued `WM_KEYDOWN` for the window that is about to
/// gain focus before it dispatches the matching `WM_SETFOCUS`, so the first
/// keystroke of an OS-level window switch (e.g. Alt+Tab's `Tab`) arrives
/// while `winit`'s cached focus state still reads unfocused.
#[cfg(target_os = "windows")]
fn is_pre_focus_key_leak(state: ElementState, window_has_focus: bool) -> bool {
    state == ElementState::Pressed && !window_has_focus
}

/// Whether `about_to_wait` should back off to `PTY_POLL_INTERVAL_IDLE`:
/// `PTY_ACTIVE_WINDOW` has elapsed since `last_activity` as of `now`.
fn is_poll_idle(last_activity: Instant, now: Instant) -> bool {
    now.saturating_duration_since(last_activity) >= PTY_ACTIVE_WINDOW
}

// ========================================================================
// Data Structures
// ========================================================================

/// The application state, driven by winit's `ApplicationHandler` trait.
pub struct App {
    /// Tabbar element currently under the cursor; drives hover highlights.
    pub(crate) tabbar_hover: TabbarHit,
    pub(crate) tab_hover_pos: Option<(f32, f32)>,
    pub(crate) config: Config,
    /// Filesystem-event receiver for the config directory: drained each
    /// `about_to_wait` tick to hot-reload as soon as a config/theme file is
    /// saved, rather than on a poll interval. `None` if the platform's watch
    /// backend failed to start, in which case hot-reload is simply disabled.
    pub(crate) config_watch_rx: Option<mpsc::Receiver<()>>,
    /// Owns the filesystem watch backing `config_watch_rx`; never read again
    /// after construction, but must stay alive for as long as the app runs —
    /// dropping it stops event delivery.
    pub(crate) _config_watcher: Option<notify::RecommendedWatcher>,
    pub(crate) cursor_pos: (f32, f32),
    pub(crate) dirty: bool,
    /// Next instant at which a held-button selection drag near the top/bottom
    /// viewport edge is allowed to auto-scroll by another line. Throttles
    /// [`App::auto_scroll_selection`] against the ~16ms `about_to_wait` tick.
    pub(crate) auto_scroll_next: Instant,
    /// Previous cursor pixel position during a split-divider drag, or `None`
    /// when no drag is in progress. Cleared on mouse release.
    pub(crate) divider_drag: Option<(f32, f32)>,
    /// Which pane's scrollbar is being dragged, if any. Cleared on mouse release.
    pub(crate) scrollbar_drag: Option<PaneId>,
    /// A transient status-bar notice and the instant it expires: an error (e.g.
    /// a Vim edit aimed at the non-editable scrollback area) or an info
    /// confirmation (e.g. "Copied to clipboard").
    pub(crate) notice: Option<(String, NoticeKind, Instant)>,
    /// A config diagnostic from startup, held until the window exists.
    /// A notice raised in `App::new` would start its expiry clock before GPU
    /// init and could lapse before the first frame ever paints.
    pub(crate) pending_config_error: Option<String>,
    /// A single long-lived clipboard handle, created on first use. On Linux the
    /// process must keep the clipboard owner alive to serve the contents to
    /// other apps; a fresh handle dropped right after writing loses them, so
    /// every copy/paste reuses this one instance.
    pub(crate) clipboard: Option<arboard::Clipboard>,
    pub(crate) folded_blocks: HashMap<PaneId, HashSet<usize>>,
    /// Raster-image blocks rendered natively on the GPU (instead of a WebView).
    pub(crate) image_blocks: Vec<ImageBlock>,
    pub(crate) last_click: Option<(Instant, f32, f32)>,
    /// Set when the window gains focus; cleared on the next `ModifiersChanged`
    /// event. While set, `KeyboardInput::Pressed` events are swallowed because
    /// winit synthesizes presses for all physically-held keys on `XI_FocusIn`,
    /// and those would otherwise be forwarded to the PTY (e.g. Tab from Alt+Tab).
    pub(crate) suppress_synthesized_keys: bool,
    /// Whether the OS window currently has keyboard focus. `false` while the
    /// user has switched away (alt-tab, another app clicked to front), which
    /// draws the focused pane's cursor in its unfocused form — a block cursor
    /// as a hollow outline, a bar or underline cursor faded toward the
    /// background — matching the convention most terminals use to signal
    /// "not receiving your keystrokes right now".
    pub(crate) window_focused: bool,
    /// The last char-search (`f`/`F`/`t`/`T`), repeated by `;` and `,`.
    pub(crate) last_find: Option<input::FindChar>,
    pub(crate) last_tile_layout: Option<(usize, usize, u32, u32)>,
    pub(crate) modifiers: winit::event::Modifiers,
    pub(crate) mouse_down: bool,
    /// Set when the custom window-close control is clicked, drained by the mouse
    /// handler into the same quit path as a native close request.
    pub(crate) exit_requested: bool,
    /// Set by the "reload" command or an incoming [`ControlMessage::Reload`],
    /// drained in `about_to_wait` into [`App::reload`] instead of [`App::quit`].
    pub(crate) pending_reload: bool,
    /// Background thread's control-channel messages (see [`crate::control`]),
    /// polled in `about_to_wait`. `None` when another instance already owns
    /// the control socket, so this instance doesn't accept reload requests.
    pub(crate) control_rx: Option<mpsc::Receiver<ControlMessage>>,
    /// Per-pane Normal-mode traversal cursor, in viewport `(row, col)`. An entry
    /// exists only while that pane is in Normal/Visual mode. Stored per pane (not
    /// as a single shared field) so switching focus between panes can never leak
    /// one pane's cursor coordinates into another — the bug that previously left
    /// the cursor sitting before the destination pane's first typeable column.
    pub(crate) nav_cursors: HashMap<PaneId, (usize, usize)>,
    /// Set after a Vim prompt edit so the nav cursor is re-seeded to the shell
    /// cursor once the resulting PTY echo is drained.
    pub(crate) nav_resync_pending: bool,
    pub(crate) next_image_id: u64,
    pub(crate) panes: HashMap<PaneId, Pane>,
    /// Panes whose last PTY-forwarded Insert-mode key was Tab - likely mid the
    /// shell's own tab-completion (e.g. zsh's menu-select). Lets the next bare
    /// Escape reach the PTY and cancel it, instead of switching straight to
    /// Normal mode underneath a completion the shell never got a chance to
    /// close - see `escape_forwarded_to_pty` and its use in `window_event`.
    pub(crate) pending_tab_completion: HashSet<PaneId>,
    /// The pane and instant of the last bare Escape that was forwarded to a
    /// full-screen app instead of switching to Normal mode. A second bare
    /// Escape on the same pane within [`ALT_SCREEN_ESCAPE_DOUBLE_TAP`]
    /// switches to Normal mode instead of forwarding — vim's own Escape (and
    /// htop's, etc.) still gets every single press, so double-tapping is the
    /// only way in, not a hijack of the app's own key. Cleared by any other
    /// key so an Escape typed long after an unrelated keystroke never counts
    /// as the second tap.
    pub(crate) last_alt_screen_escape: Option<(PaneId, Instant)>,
    /// Per-pane shadow model of the editable prompt line, powering `Ctrl-/`
    /// undo and `Ctrl-\` redo at the shell prompt.
    pub(crate) prompt_shadows: HashMap<PaneId, PromptShadow>,
    pub(crate) palette: Option<Palette>,
    pub(crate) pane_titles: HashMap<PaneId, String>,
    /// User-set custom names for tabs, keyed by tab index. Take priority over
    /// OSC-set titles. Indices are shifted down when a tab before them is closed.
    pub(crate) tab_names: HashMap<usize, String>,
    /// In-progress tab rename input, set while the user is typing a new name.
    pub(crate) tab_rename_input: Option<String>,
    /// In-progress "create new theme" name input, set while the user is
    /// typing a name for the theme file about to be saved.
    pub(crate) theme_name_input: Option<String>,
    pub(crate) pending: PendingPrefix,
    /// When the current `pending` prefix was opened, for which-key hint delay.
    pub(crate) pending_since: Option<Instant>,
    pub(crate) quick_select: Option<Vec<QuickLabel>>,
    /// The `f`/`F`/`t`/`T` jump overlay: one label per candidate landing spot in the
    /// focused pane, shown when the target character occurs more than once so the
    /// user picks one with a single keystroke instead of repeating `;`.
    pub(crate) find_labels: Option<Vec<FindLabel>>,
    pub(crate) renderer: Option<GpuRenderer>,
    pub(crate) search_query: Option<String>,
    /// 1-based index of the focused match (0 when no matches).
    pub(crate) search_match_index: usize,
    pub(crate) search_match_total: usize,
    /// Where the active search was launched from: the pane and the absolute
    /// buffer position (`(row, col)`) of the cursor when `/`/`?`/`*` was
    /// pressed. Every keystroke of the query re-searches from here, so typing
    /// doesn't creep forward through the buffer.
    pub(crate) search_origin: Option<(PaneId, (usize, usize))>,
    /// The match `n`/`N` is parked on — the pane and the match's absolute
    /// `(row, col)` start. Drawn in [`winter_render::Theme::search_current_bg`]
    /// so the focused match stands out from the other highlighted matches.
    pub(crate) search_current: Option<(PaneId, (usize, usize))>,
    /// The last query searched for, kept after the search is put away with `Esc`
    /// so `n`/`N` can pick it back up from wherever the cursor now is — vim keeps
    /// the pattern the same way across `:nohlsearch`.
    pub(crate) search_last: Option<String>,
    /// Vim-style search direction: `?`/`#` set this so `n` repeats backward and
    /// `N` forward (both reversed from the `/`/`*` default). `SearchNext`
    /// (`n`) walks in this direction; `SearchPrevious` (`N`) walks the other.
    pub(crate) search_reverse: bool,
    pub(crate) selection: Option<Selection>,
    /// The open full-window settings page (a native text-mode overlay), or
    /// `None`. Rendered by [`render`] as a terminal grid covering the window.
    pub(crate) settings_page: Option<SettingsPage>,
    /// The cursor position before Buffer Swoop opened, restored on cancel.
    pub(crate) swoop_initial_cursor: Option<(PaneId, (usize, usize))>,
    /// Which tab is shown; index into [`Self::tabs`].
    pub(crate) active_tab: usize,
    /// Tab indices in most-recently-used order (front = the current tab after a
    /// deliberate switch). Drives the recency tab commands.
    pub(crate) tab_mru: Vec<usize>,
    /// Cursor into [`Self::tab_mru`] while a recency walk is in progress, so
    /// repeated recency commands step through usage order without reshuffling it.
    /// `None` once a deliberate switch ends the walk.
    pub(crate) mru_walk: Option<usize>,
    /// Per-pane vim-style jumplists backing `Ctrl+O`/`Ctrl+I` (see
    /// [`navigation::JumpList`]).
    pub(crate) jump_lists: HashMap<PaneId, navigation::JumpList>,
    /// Per-pane vim-style changelists backing `g;`/`g,` (see
    /// [`navigation::ChangeList`]).
    pub(crate) change_lists: HashMap<PaneId, navigation::ChangeList>,
    /// The most recent change per pane, replayed by `.` (see
    /// [`navigation::LastChange`]).
    pub(crate) last_changes: HashMap<PaneId, navigation::LastChange>,
    /// The Insert-mode typing run in progress per pane (see
    /// [`navigation::InsertSession`]).
    pub(crate) insert_sessions: HashMap<PaneId, navigation::InsertSession>,
    /// Per-pane named marks (`(PaneId, mark_char) -> (abs_row, col)`).
    pub(crate) marks: HashMap<(PaneId, char), (usize, usize)>,
    /// Vim named registers (`"{a-z}`, `"{0-9}`, `"+`, `"*`).
    pub(crate) registers: HashMap<char, String>,
    /// Previously executed palette queries, persisted across sessions.
    pub(crate) palette_history: Vec<String>,
    /// The last Visual selection, restored by `gv` (see [`LastVisual`]).
    pub(crate) last_visual: Option<LastVisual>,
    /// The open menu/dropdown (index into the tabbar's menu list), or `None`.
    pub(crate) open_menu: Option<usize>,
    /// Index of the open submenu's parent within the open menu's items, or `None`.
    pub(crate) open_submenu: Option<usize>,
    /// The hovered dropdown item while a menu is open.
    pub(crate) selected_item: Option<usize>,
    /// The hovered submenu child while a submenu is open.
    pub(crate) selected_subitem: Option<usize>,
    /// Next free globally-unique pane id; allocated by [`Self::alloc_pane_id`].
    pub(crate) next_pane_id: u64,
    /// All open tabs, each its own split-tree of panes.
    pub(crate) tabs: Vec<Tab>,
    pub(crate) modes: HashMap<PaneId, Mode>,
    /// Visual-mode anchor (viewport `(row, col)`) where the selection began.
    /// `Some` only while the focused pane is in Visual mode.
    pub(crate) visual_anchor: Option<(usize, usize)>,
    /// The active Visual selection kind (Block, Char, Line).
    pub(crate) visual_kind: VisualKind,
    pub(crate) webview_mgr: WebViewManager,
    pub(crate) window: Option<Arc<Window>>,
    /// Configurable split/close/focus key bindings (the `window` keybindings
    /// block), resolved against in Normal mode.
    pub(crate) window_keymap: WindowKeymap,
    pub(crate) window_title: String,
    /// The URL of the hyperlinked cell currently under the pointer, if any.
    /// Drives the pointer-cursor icon and Ctrl+click to open.
    pub(crate) hovered_url: Option<String>,
    /// Active right-click context menu: pixel position where it was opened.
    pub(crate) context_menu_pos: Option<(f32, f32)>,
    /// The URL (if any) that was under the pointer when the context menu opened.
    pub(crate) context_menu_url: Option<String>,
    /// The actions bound to each context-menu item, parallel to the rendered list.
    pub(crate) context_menu_actions: Vec<ContextAction>,
    /// The currently hovered item in the context menu (drives the hover highlight).
    pub(crate) context_menu_selected: Option<usize>,
    /// The font size from config at startup; Ctrl+0 resets back to this value.
    pub(crate) base_font_size: f32,
    /// Whether the cursor is in its "visible" blink phase. Toggled by the blink
    /// timer; always `true` while blink is disabled.
    pub(crate) blink_phase: bool,
    /// When the next blink phase flip is due. Updated on every flip and on key
    /// press (which resets the cursor to visible to give feedback).
    pub(crate) blink_next_flip: Instant,
    /// When PTY output, a window event, or any other activity was last
    /// observed. Drives the `about_to_wait` idle back-off (see
    /// `PTY_ACTIVE_WINDOW`).
    pub(crate) last_activity: Instant,
    /// Source tab index and the pointer x position when a tab drag began. `None`
    /// when no drag is in progress. Cleared on mouse release.
    pub(crate) tab_drag_start: Option<(usize, f32)>,
}

/// An action dispatched from a right-click context menu item.
#[derive(Clone)]
pub(crate) enum ContextAction {
    Copy,
    OpenLink(String),
    Paste,
}

/// A block drawn natively via the GPU. `id` keys the renderer's texture cache;
/// `nat_w`/`nat_h` are the rendered pixel dimensions, used to preserve aspect
/// ratio when placing it at `grid_row`. Width-wrapped blocks carry their source
/// in `reflow` so they can be re-rasterized at `rastered_width` on resize.
pub(crate) struct ImageBlock {
    /// Scrollback block-list position of the block this image renders, used
    /// to match live-block patch refreshes to their texture.
    pub block_index: usize,
    /// Set once the live block behind this image closed; dims the placed
    /// quad instead of drawing it at full opacity.
    pub closed: bool,
    /// True for images/SVG (scaled down to fit the reserved band); false for
    /// text/markdown (shown at native size and clipped to the band).
    pub fit_to_band: bool,
    pub grid_row: usize,
    pub id: u64,
    /// Band height in rows the block is drawn into — matches the rows reserved
    /// for it in the grid, so the following prompt sits flush below.
    pub max_rows: usize,
    pub nat_h: u32,
    pub nat_w: u32,
    pub pane_id: PaneId,
    pub rastered_width: u32,
    pub reflow: Option<ReflowSource>,
    /// Segment position within the block's output list, paired with
    /// `block_index` for live-block matching.
    pub segment_index: usize,
}

/// Source content for a width-wrapped native block, retained so the block can
/// be re-rasterized when the pane width changes.
pub(crate) enum ReflowSource {
    Markdown(String),
    Text(String),
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

#[derive(Clone, Debug)]
pub(crate) struct Selection {
    pub block: bool,
    pub end_col: usize,
    pub end_row: usize,
    pub pane: PaneId,
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
    pub pane: PaneId,
}

// ========================================================================
// App
// ========================================================================

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let mut modes = HashMap::new();
        modes.insert(PaneId(0), Mode::default());

        let (config, config_error) = Config::load_checked();
        let window_keymap = WindowKeymap::from_config(
            config.keybindings.get("window"),
            config.keybindings.get("editing"),
        );
        let (config_watcher, config_watch_rx) = match crate::config::watch::spawn_watcher() {
            Some((watcher, rx)) => (Some(watcher), Some(rx)),
            None => (None, None),
        };
        let base_font_size = config.font_size;

        Self {
            tabbar_hover: TabbarHit::None,
            tab_hover_pos: None,
            config,
            config_watch_rx,
            _config_watcher: config_watcher,
            cursor_pos: (0.0, 0.0),
            dirty: true,
            auto_scroll_next: Instant::now(),
            divider_drag: None,
            scrollbar_drag: None,
            notice: None,
            pending_config_error: config_error,
            clipboard: None,
            folded_blocks: HashMap::new(),
            last_click: None,
            suppress_synthesized_keys: false,
            window_focused: true,
            last_find: None,
            image_blocks: Vec::new(),
            last_tile_layout: None,
            modifiers: winit::event::Modifiers::default(),
            mouse_down: false,
            exit_requested: false,
            pending_reload: false,
            control_rx: control::spawn_listener(),
            nav_cursors: HashMap::new(),
            nav_resync_pending: false,
            next_image_id: 0,
            panes: HashMap::new(),
            pending_tab_completion: HashSet::new(),
            last_alt_screen_escape: None,
            prompt_shadows: HashMap::new(),
            palette: None,
            pane_titles: HashMap::new(),
            tab_names: HashMap::new(),
            tab_rename_input: None,
            theme_name_input: None,
            pending: PendingPrefix::None,
            pending_since: None,
            quick_select: None,
            find_labels: None,
            renderer: None,
            search_query: None,
            search_match_index: 0,
            search_match_total: 0,
            search_current: None,
            search_last: None,
            search_origin: None,
            search_reverse: false,
            selection: None,
            settings_page: None,
            swoop_initial_cursor: None,
            active_tab: 0,
            tab_mru: vec![0],
            mru_walk: None,
            jump_lists: HashMap::new(),
            change_lists: HashMap::new(),
            last_changes: HashMap::new(),
            insert_sessions: HashMap::new(),
            marks: HashMap::new(),
            registers: HashMap::new(),
            palette_history: crate::config::load_state().palette_history,
            last_visual: None,
            open_menu: None,
            open_submenu: None,
            selected_item: None,
            selected_subitem: None,
            next_pane_id: 1,
            tabs: vec![Tab::new()],
            modes,
            visual_anchor: None,
            visual_kind: VisualKind::Char,
            webview_mgr: WebViewManager::new(),
            window: None,
            window_keymap,
            window_title: String::new(),
            hovered_url: None,
            context_menu_pos: None,
            context_menu_url: None,
            context_menu_actions: Vec::new(),
            context_menu_selected: None,
            base_font_size,
            blink_phase: true,
            blink_next_flip: Instant::now() + CURSOR_BLINK_PERIOD,
            last_activity: Instant::now(),
            tab_drag_start: None,
        }
    }

    /// The currently visible tab.
    pub(crate) fn tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }

    /// The currently visible tab, mutably.
    pub(crate) fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_tab]
    }

    /// Allocate the next globally-unique pane id.
    pub(crate) fn alloc_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        id
    }

    /// Drain pending config-directory filesystem events and, if any arrived,
    /// reload and apply the new settings, including re-resolving the theme's
    /// colors. A single save can raise several events (e.g. an editor's
    /// write-temp-then-rename), so every pending one is drained first and
    /// applied as one reload rather than one per event. Returns true when a
    /// reload occurred.
    pub(crate) fn reload_config_if_changed(&mut self) -> bool {
        let Some(rx) = &self.config_watch_rx else {
            return false;
        };
        let mut changed = false;
        while rx.try_recv().is_ok() {
            changed = true;
        }
        if !changed {
            return false;
        }
        let (config, config_error) = Config::load_checked();
        self.config = config;
        if let Some(message) = config_error {
            self.set_error(message);
        }
        self.window_keymap = WindowKeymap::from_config(
            self.config.keybindings.get("window"),
            self.config.keybindings.get("editing"),
        );
        let ligatures = self.config.ligatures;
        let pane_border_width = self.config.pane_border_width;
        if let Some(r) = &mut self.renderer {
            r.set_ligatures(ligatures);
            r.set_divider_width(pane_border_width);
        }
        if !self.config.cursor.blink {
            self.blink_phase = true;
        }
        // An edited `window-title-template` in settings.kdl takes effect
        // without restart.
        self.update_window_title();
        self.rebuild_theme();
        self.resize_all_panes();
        true
    }

    /// Number of top cell rows reserved for the tabbar/menubar.
    pub(crate) fn top_chrome_rows(&self) -> usize {
        winter_render::tabbar_rows(self.config.menu_style)
    }

    /// The outer-edge/corner this point should resize when pressed, or `None`
    /// away from the border. Only the Modern title bar needs this: the System
    /// style keeps the OS decorations, which already carry a resize border.
    pub(crate) fn edge_resize_direction(&self, x: f32, y: f32) -> Option<ResizeDirection> {
        if self.config.title_bar_style != TitleBarStyle::Modern {
            return None;
        }
        let size = self.window.as_ref()?.inner_size();
        edge_resize_direction_at(
            x,
            y,
            size.width as f32,
            size.height as f32,
            WINDOW_RESIZE_BORDER_PX,
        )
    }

    /// Whether the status bar is actually shown this frame: either configured
    /// on, or forced on for the duration of a live `/` search, which is the
    /// only place search feedback (query text, match position) is displayed.
    /// Shared by `viewport_rect`/`resize_all_panes` (pane geometry and PTY
    /// size) and `render_frame` (what's drawn), so they always agree on how
    /// much space is reserved at the bottom of the window.
    pub(crate) fn status_bar_visible(&self) -> bool {
        self.config.status_bar.enabled || self.search_query.is_some()
    }

    pub(crate) fn viewport_rect(&self) -> PaneRect {
        let (w, h) = match (&self.window, &self.renderer) {
            (Some(win), _) => {
                let size = win.inner_size();
                (size.width as f32, size.height as f32)
            }
            (None, Some(r)) => {
                let (cols, rows) = r.grid_size();
                let (cw, ch) = r.cell_size();
                (cols as f32 * cw, rows as f32 * ch)
            }
            (None, None) => (800.0, 600.0),
        };
        // Reserve the status bar row at the bottom (when enabled) and the
        // tabbar/menubar rows at the top, so pane hit-testing and focus geometry
        // match the area actually drawn to panes; the grid is centered in
        // whatever space remains below the tabbar (must match `render_frame`).
        let ch = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size().1)
            .unwrap_or(0.0);
        let top_rows = self.top_chrome_rows();
        let status_enabled = self.status_bar_visible();

        let top_h_on_screen = if self.config.menu_style == MenuStyle::Modern {
            winter_render::modern_tabbar_height_px(ch)
        } else {
            top_rows as f32 * ch
        };
        let status_h = if status_enabled {
            winter_render::STATUS_BAR_HEIGHT * ch
        } else {
            0.0
        };

        // Floor to whole cell rows and center the leftover sub-row slack above
        // and below the pane band, whether or not the status bar eats into it,
        // so a window height that isn't an exact multiple of the cell height
        // never leaves a dead, un-drawable strip pinned to one edge.
        let (content_rows, top_padding) = content_band(h - top_h_on_screen - status_h, ch);
        PaneRect {
            x: 0.0,
            y: top_h_on_screen + top_padding,
            width: w,
            height: (content_rows as f32 * ch).max(1.0),
        }
    }

    /// The pane area as a layout `Rect` (same coordinates as [`PaneRect`]).
    pub(crate) fn content_viewport(&self) -> Rect {
        let vp = self.viewport_rect();
        Rect::new(vp.x, vp.y, vp.width, vp.height)
    }

    /// While a selection drag is held near the top/bottom edge of the content
    /// viewport, scroll the selection's pane one line into (or back out of)
    /// history and extend the drag's live end to the new edge row, so text
    /// outside the current page can be reached, and pulled into the
    /// selection, without the pointer leaving the window. Because `Selection`
    /// rows are absolute ([`winter_render::Grid::to_absolute_row`]),
    /// re-deriving the edge row from the post-scroll view on every call grows
    /// `end_row` further each time rather than snapping back to a fixed
    /// viewport position. Scrolling itself is throttled to one line per
    /// [`AUTO_SCROLL_INTERVAL`] via `auto_scroll_next`; a no-op when the
    /// button isn't held, there's no active selection, or the pointer isn't
    /// within [`AUTO_SCROLL_EDGE_MARGIN`] of an edge.
    pub(crate) fn auto_scroll_selection(&mut self) {
        if !self.mouse_down {
            return;
        }
        let Some(pane_id) = self.selection.as_ref().map(|sel| sel.pane) else {
            return;
        };
        let vp = self.viewport_rect();
        let (_, y) = self.cursor_pos;
        let scroll_up = if y < vp.y + AUTO_SCROLL_EDGE_MARGIN {
            true
        } else if y > vp.y + vp.height - AUTO_SCROLL_EDGE_MARGIN {
            false
        } else {
            return;
        };
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return;
        };

        // Speed scales with how deep into the edge margin the pointer sits:
        // just inside crawls, at the margin's full depth (the viewport's own
        // edge) hits AUTO_SCROLL_MAX_LINES_PER_TICK. A pointer past the margin
        // (above the viewport entirely) clamps at that same cap.
        let depth = if scroll_up {
            vp.y + AUTO_SCROLL_EDGE_MARGIN - y
        } else {
            y - (vp.y + vp.height - AUTO_SCROLL_EDGE_MARGIN)
        };
        let extra = (depth / AUTO_SCROLL_EDGE_MARGIN * (AUTO_SCROLL_MAX_LINES_PER_TICK - 1) as f32)
            .floor() as usize;
        let lines = (1 + extra).min(AUTO_SCROLL_MAX_LINES_PER_TICK);

        let now = Instant::now();
        if now >= self.auto_scroll_next {
            self.auto_scroll_next = now + AUTO_SCROLL_INTERVAL;
            let grid = pane.grid_mut();
            if scroll_up {
                grid.scroll_up_history(lines);
            } else {
                grid.scroll_down_history(lines);
            }
        }

        let grid = pane.grid();
        let (edge_row, edge_col) = if scroll_up {
            (0, 0)
        } else {
            (grid.rows().saturating_sub(1), grid.cols().saturating_sub(1))
        };
        let abs_edge_row = grid.to_absolute_row(edge_row);
        if let Some(sel) = &mut self.selection {
            sel.end_row = abs_edge_row;
            sel.end_col = edge_col;
        }
        // The view scrolls under a held pointer during edge auto-scroll, so the
        // traversal cursor must move to the drag's live end too — otherwise it
        // would freeze mid-screen while the selection grows past it.
        self.track_nav_cursor_to_mouse(pane_id, edge_row, edge_col);
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    pub(crate) fn layout_rect_to_pane(rect: Rect) -> PaneRect {
        // Snap the pane to the pixel grid by rounding its top-left AND
        // bottom-right corners, then deriving width/height from the rounded
        // corners. Rounding width/height independently (the obvious
        // alternative) lets two siblings of a split disagree about their shared
        // edge by 1 px — because `round(x) + round(w) != round(x + w)` — which
        // both drops the divider between them (the shared edge no longer
        // coincides) and opens a 1 px gap at divider crossings. Corner-rounding
        // keeps the shared edge exact: a split produces
        // `first.x + first.width == second.x` in the unrounded tree, so rounding
        // that same value for both panes gives identical pixel edges.
        let x = rect.x.round();
        let y = rect.y.round();
        let right = (rect.x + rect.width).round();
        let bottom = (rect.y + rect.height).round();
        PaneRect {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }

    /// Render the `window-title-template` config against the active tab: the
    /// app running in the focused pane feeds the `{{ title }}`,
    /// `{{ app_name }}`, `{{ pane_title }}`, and `{{ cwd }}` placeholders.
    pub(crate) fn render_window_title(&self) -> String {
        let idx = self.active_tab;
        let focused = self.tabs[idx].focused();
        let pane = self.panes.get(&focused);
        expand_window_title_template(
            &self.config.window_title_template,
            &WindowTitleVars {
                title: self.tab_title(idx),
                app_name: pane
                    .and_then(|p| p.foreground_process_name())
                    .unwrap_or_default(),
                pane_title: self
                    .pane_titles
                    .get(&focused)
                    .map(|t| tabbar::clean_title(t))
                    .unwrap_or_default(),
                cwd: pane
                    .and_then(|p| p.cwd().map(|c| tabbar::clean_cwd(&c)))
                    .unwrap_or_default(),
            },
        )
    }

    pub(crate) fn update_window_title(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };

        let title = if self.settings_page.is_some() {
            "Winter - Settings".to_string()
        } else if let Some(palette) = &self.palette {
            let selected = palette.selected_action().unwrap_or("");
            format!("Winter - palette: > {} [{}]", palette.query, selected)
        } else if self.search_query.is_some() || self.pending == PendingPrefix::SearchInput {
            let query = self.search_query.as_deref().unwrap_or("");
            let prefix = if self.search_reverse { '?' } else { '/' };
            format!("Winter - search: {prefix}{query}")
        } else if self.quick_select.is_some() {
            "Winter - quick select".to_string()
        } else if let winter_render::TabbarHit::Tab(idx) = self.tabbar_hover {
            let t = self.tab_title(idx);
            format!("Winter - {t}")
        } else {
            self.render_window_title()
        };

        // The window manager call is an IPC round-trip; only make it when the
        // title actually changes, not on every keystroke or PTY poll.
        if title != self.window_title {
            window.set_title(&title);
            self.window_title = title;
        }
    }

    /// Adjust font size by `delta` points; returns true when actually changed.
    pub(crate) fn change_font_size(&mut self, delta: f32) -> bool {
        let new_size = (self.config.font_size + delta).clamp(6.0, 72.0);
        self.change_font_size_to(new_size)
    }

    /// Set font size to `logical_size` points; returns true when actually changed.
    pub(crate) fn change_font_size_to(&mut self, logical_size: f32) -> bool {
        let Some(renderer) = &mut self.renderer else {
            return false;
        };
        if renderer.set_font_size(logical_size).is_none() {
            return false;
        }
        self.config.font_size = logical_size;
        self.resize_all_panes();
        self.dirty = true;
        true
    }

    /// Show `message` as a transient error notice (red) in the status bar.
    pub(crate) fn set_error(&mut self, message: impl Into<String>) {
        self.show_notice(message, NoticeKind::Error);
    }

    /// Show `message` as a transient info notice (green) in the status bar, used
    /// to confirm an action such as copying text to the clipboard.
    pub(crate) fn set_notice(&mut self, message: impl Into<String>) {
        self.show_notice(message, NoticeKind::Info);
    }

    /// Store a transient notice of `kind` and force a redraw so it paints now
    /// and clears once it expires.
    fn show_notice(&mut self, message: impl Into<String>, kind: NoticeKind) {
        self.notice = Some((message.into(), kind, Instant::now() + NOTICE_DURATION));
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Spawns a pane, retrying with the OS default shell (ignoring `shell`)
    /// if the first attempt fails, so a bad `shell`/`shell-*` setting or a
    /// session-restore command that no longer exists surfaces as a
    /// status-bar error instead of crashing the app. `None` only when the
    /// fallback also fails (e.g. the OS itself is out of resources), in
    /// which case the caller must not proceed as if a pane was created.
    fn spawn_pane_or_notify(
        &mut self,
        cols: usize,
        rows: usize,
        shell: Option<&str>,
        scrollback: usize,
        cwd: Option<&str>,
    ) -> Option<Pane> {
        match Pane::new_with_cwd(cols, rows, shell, scrollback, cwd) {
            Ok(pane) => Some(pane),
            Err(e) => {
                self.set_error(format!(
                    "shell failed to start ({e}), trying the default shell"
                ));
                match Pane::new_with_cwd(cols, rows, None, scrollback, cwd) {
                    Ok(pane) => Some(pane),
                    Err(e2) => {
                        self.set_error(format!("could not start a shell: {e2}"));
                        None
                    }
                }
            }
        }
    }

    /// The current notice text and kind, if one is set and has not yet expired.
    pub(crate) fn active_notice(&self) -> Option<(&str, NoticeKind)> {
        self.notice
            .as_ref()
            .filter(|(_, _, expiry)| Instant::now() < *expiry)
            .map(|(text, kind, _)| (text.as_str(), *kind))
    }

    /// The working directory of the currently focused pane of the active tab, if
    /// available. Used to spawn a new pane/tab in the same directory.
    fn focused_cwd(&self) -> Option<String> {
        let focused = self.tab().focused();
        self.panes.get(&focused).and_then(|pane| pane.cwd())
    }

    pub(crate) fn split_pane(&mut self, direction: Direction) {
        let new_id = self.alloc_pane_id();
        // Capture the focused pane's cwd before the layout split so the new pane
        // opens in the same working directory instead of the process default.
        let cwd = self.focused_cwd();
        self.tab_mut().split(direction, SPLIT_RATIO, new_id);
        // Rebalance every split's ratio so all panes share the viewport equally,
        // without altering the tree shape the user built (mixed split
        // directions stay mixed; only the sizes change). See `Tab::balance`.
        self.tab_mut().balance();

        let (pane_cols, pane_rows) = self.spawn_grid_size(new_id, direction);
        let shell = self.config.active_shell().map(String::from);
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        let Some(pane) = self.spawn_pane_or_notify(
            pane_cols.max(1),
            pane_rows.max(1),
            shell.as_deref(),
            scrollback,
            cwd.as_deref(),
        ) else {
            // Undo the split: no pane exists for `new_id`, so the tree can't
            // be left referencing it.
            self.tab_mut().close(new_id);
            self.tab_mut().balance();
            self.dirty = true;
            return;
        };
        self.panes.insert(new_id, pane);
        self.modes.insert(new_id, Mode::default());

        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }

    /// Grid size to spawn `pane` at: the exact size of its post-split rect
    /// when a renderer is available, else the legacy half-window guess.
    ///
    /// Spawning at the real size matters on Windows: the old estimate used
    /// `renderer.grid_size()` (the full window grid, including the
    /// tabbar/status-bar rows and ignoring existing splits), so the child was
    /// routinely started too large — by the chrome rows for a first split, and
    /// roughly 2× too tall when splitting an already-half-height pane.
    /// [`Self::resize_all_panes`] would then shrink the PTY + grid to fit. Unix
    /// PTYs reflow that shrink cleanly, but Windows ConPTY reflows a shrink
    /// asynchronously and lossily, so a shell that draws at startup (e.g.
    /// nushell's banner/prompt) briefly paints for the oversized grid and lands
    /// offset within the pane until it redraws — looking like the split missed
    /// the middle. Sizing the child correctly up front means its first render is
    /// already at the final size, so [`Self::resize_all_panes`] has nothing to
    /// shrink (its `Pane::resize` early-returns on the unchanged size).
    fn spawn_grid_size(&self, pane: PaneId, direction: Direction) -> (usize, usize) {
        let Some(renderer) = self.renderer.as_ref() else {
            let (cols, rows) = (DEFAULT_COLS as usize, DEFAULT_ROWS as usize);
            return match direction {
                Direction::Vertical => (cols / 2, rows),
                Direction::Horizontal => (cols, rows / 2),
            };
        };
        let viewport = self.content_viewport();
        if let Some((_, rect)) = self
            .tab()
            .rects(viewport)
            .into_iter()
            .find(|(id, _)| *id == pane)
        {
            return renderer.grid_size_for(Self::layout_rect_to_pane(rect));
        }
        // `pane` was just inserted by `split`, so the lookup above always
        // succeeds; this only guards a hypothetical caller that splits before
        // the layout is consistent.
        let (cols, rows) = renderer.grid_size();
        match direction {
            Direction::Vertical => (cols / 2, rows),
            Direction::Horizontal => (cols, rows / 2),
        }
    }

    pub(crate) fn close_pane(&mut self, pane_id: PaneId) {
        self.close_pane_in_any_tab(pane_id);
    }

    /// Close `pane_id` in whichever tab holds it, collapsing its split into the
    /// sibling. When it is the last pane in its tab, the whole tab is closed.
    /// Drops all per-pane state and re-lays-out if the affected tab is the active one.
    fn close_pane_in_any_tab(&mut self, pane_id: PaneId) {
        let Some(tab_idx) = self.tabs.iter().position(|t| t.panes().contains(&pane_id)) else {
            return;
        };
        if self.tabs[tab_idx].panes().len() <= 1 {
            // Closing the last pane of a tab closes that tab — but never the last
            // pane of the only tab, so the close-pane command always leaves at
            // least one pane open.
            if self.tabs.len() <= 1 {
                return;
            }
            self.close_tab(tab_idx);
            return;
        }
        self.panes.remove(&pane_id);
        self.modes.remove(&pane_id);
        self.nav_cursors.remove(&pane_id);
        self.jump_lists.remove(&pane_id);
        self.change_lists.remove(&pane_id);
        self.last_changes.remove(&pane_id);
        self.insert_sessions.remove(&pane_id);
        self.marks.retain(|(p, _), _| *p != pane_id);
        self.pane_titles.remove(&pane_id);
        self.webview_mgr.remove_tiles_for_pane(pane_id);
        self.image_blocks.retain(|img| img.pane_id != pane_id);
        self.last_tile_layout = None;
        self.tabs[tab_idx].close(pane_id);
        // Rebalance the remaining panes' ratios so they stay evenly spaced,
        // without reshaping the tree (closing one pane would otherwise leave
        // its sibling oversized).
        self.tabs[tab_idx].balance();
        if tab_idx == self.active_tab && self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }

    /// Close every pane in the tab except `focused` (Vim `Ctrl-w o`).
    pub(crate) fn close_other_panes(&mut self, focused: PaneId) {
        let others: Vec<PaneId> = self
            .tab()
            .panes()
            .into_iter()
            .filter(|&id| id != focused)
            .collect();
        for id in others {
            self.close_pane(id);
        }
    }

    /// Open a new tab with a fresh shell pane and switch to it.
    pub(crate) fn new_tab(&mut self) {
        let id = self.alloc_pane_id();
        // Open the new tab in the focused pane's working directory rather than
        // the process default (usually `$HOME`).
        let cwd = self.focused_cwd();
        let (cols, rows) = self
            .renderer
            .as_ref()
            .map(|r| r.grid_size())
            .unwrap_or((DEFAULT_COLS as usize, DEFAULT_ROWS as usize));
        // Sized roughly now; resize_all_panes fixes the exact grid once placed.
        let shell = self.config.active_shell().map(String::from);
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        let Some(pane) = self.spawn_pane_or_notify(
            cols.max(1),
            rows.max(1),
            shell.as_deref(),
            scrollback,
            cwd.as_deref(),
        ) else {
            return;
        };
        self.push_new_tab(id, pane);
    }

    /// Open a new foreground tab whose pane is attached to a running mux
    /// session; the session's buffered output replays into the pane.
    pub(crate) fn new_mux_tab(&mut self, session: &str) {
        self.new_mux_tab_at(&crate::mux::server::default_socket_path(), session);
    }

    /// Open a new foreground tab attached to a session on the mux server
    /// at `path`.
    fn new_mux_tab_at(&mut self, path: &str, session: &str) {
        let id = self.alloc_pane_id();
        let (cols, rows) = self
            .renderer
            .as_ref()
            .map(|r| r.grid_size())
            .unwrap_or((DEFAULT_COLS as usize, DEFAULT_ROWS as usize));
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        match Pane::new_mux_at(path, cols.max(1), rows.max(1), session, scrollback) {
            Ok(pane) => {
                self.set_notice(format!("attached to mux session '{session}'"));
                self.push_new_tab(id, pane);
            }
            Err(e) => self.set_error(format!(
                "could not attach to '{session}' ({e}); start the server with 'winter mux serve'"
            )),
        }
    }

    /// Open a new foreground tab attached to a session on a mux server
    /// reached over ssh at `host`.
    fn new_mux_tab_remote_at(&mut self, host: &str, session: &str) {
        let id = self.alloc_pane_id();
        let (cols, rows) = self
            .renderer
            .as_ref()
            .map(|r| r.grid_size())
            .unwrap_or((DEFAULT_COLS as usize, DEFAULT_ROWS as usize));
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        match Pane::new_mux_remote(host, cols.max(1), rows.max(1), session, scrollback) {
            Ok(pane) => {
                self.set_notice(format!("attached to '{host}:{session}'"));
                self.push_new_tab(id, pane);
            }
            Err(e) => self.set_error(format!("could not reach '{host}' over ssh ({e})")),
        }
    }

    /// Install an already-spawned pane as a fresh foreground tab and switch to
    /// it. Shared by `new_tab`'s shell tab and `gx`'s editor tab so both get
    /// the same bookkeeping — mode default, MRU touch, tile repositioning,
    /// resize, and title update.
    fn push_new_tab(&mut self, id: PaneId, pane: Pane) {
        self.panes.insert(id, pane);
        self.modes.insert(id, Mode::default());
        self.tabs.push(Tab::with_root(id));
        self.active_tab = self.tabs.len() - 1;
        self.touch_mru(self.active_tab);
        self.close_menu();
        self.last_tile_layout = None;
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
        self.update_window_title();
    }

    pub(crate) fn switch_to_pane(&mut self, pane_id: PaneId) {
        if let Some(tab_index) = self
            .tabs
            .iter()
            .position(|tab| tab.panes().contains(&pane_id))
        {
            self.switch_tab(tab_index);
            self.tabs[tab_index].focus(pane_id);
            self.update_window_title();
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    /// Switch the visible tab to `index` as a deliberate selection: record it as
    /// most-recently-used (ending any recency walk) and show it.
    pub(crate) fn switch_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        self.touch_mru(index);
        self.activate_tab(index);
    }

    /// Make `index` the visible tab without touching the MRU order. Shared by
    /// deliberate switches ([`Self::switch_tab`]) and recency walks
    /// ([`Self::recent_tab`]).
    fn activate_tab(&mut self, index: usize) {
        self.active_tab = index;
        self.selection = None;
        // Force a tile reposition so background-tab WebViews are hidden and the
        // new tab's are shown (the layout key alone may not have changed).
        self.last_tile_layout = None;
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
        self.update_window_title();
    }

    /// Move `index` to the front of the most-recently-used order (inserting it if
    /// new) and end any in-progress recency walk.
    fn touch_mru(&mut self, index: usize) {
        self.tab_mru.retain(|&i| i != index);
        self.tab_mru.insert(0, index);
        self.mru_walk = None;
    }

    /// Cycle to the next (`forward`) or previous tab by position, wrapping around.
    pub(crate) fn cycle_tab(&mut self, forward: bool) {
        let count = self.tabs.len();
        if count <= 1 {
            return;
        }
        let next = if forward {
            (self.active_tab + 1) % count
        } else {
            (self.active_tab + count - 1) % count
        };
        self.switch_tab(next);
    }

    /// Switch tabs in most-recently-used order: `forward` steps toward more
    /// recently used, otherwise toward less recently used, wrapping around. The
    /// MRU order is held still across consecutive calls (a "walk") so the user can
    /// step back and forth through usage history; the next deliberate switch ends
    /// the walk and re-seeds the order from the chosen tab.
    pub(crate) fn recent_tab(&mut self, forward: bool) {
        let count = self.tabs.len();
        if count <= 1 {
            return;
        }
        // Guard against any drift from tab open/close bookkeeping: a malformed
        // order is rebuilt with the current tab most-recent.
        if self.tab_mru.len() != count {
            self.tab_mru = (0..count).collect();
            self.touch_mru(self.active_tab);
        }
        let cursor = self.mru_walk.unwrap_or(0);
        let next = if forward {
            (cursor + count - 1) % count
        } else {
            (cursor + 1) % count
        };
        self.mru_walk = Some(next);
        self.activate_tab(self.tab_mru[next]);
    }

    /// Close tab `index`, dropping all its panes. The last tab is never closed.
    pub(crate) fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        if self.tabs.len() <= 1 {
            self.exit_requested = true;
            return;
        }
        for id in self.tabs[index].panes() {
            self.panes.remove(&id);
            self.modes.remove(&id);
            self.nav_cursors.remove(&id);
            self.pane_titles.remove(&id);
            self.webview_mgr.remove_tiles_for_pane(id);
            self.image_blocks.retain(|img| img.pane_id != id);
        }
        self.tabs.remove(index);
        if index < self.active_tab {
            self.active_tab -= 1;
        }
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        // Drop the closed tab from the MRU order and shift the indices above it
        // down, then re-seed the current tab as most-recent.
        self.tab_mru.retain(|&i| i != index);
        for i in self.tab_mru.iter_mut() {
            if *i > index {
                *i -= 1;
            }
        }
        // Shift custom tab names: remove the closed tab's name, shift those above it.
        self.tab_names = self
            .tab_names
            .iter()
            .filter_map(|(&i, name)| match i.cmp(&index) {
                std::cmp::Ordering::Less => Some((i, name.clone())),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some((i - 1, name.clone())),
            })
            .collect();
        self.touch_mru(self.active_tab);
        self.close_menu();
        self.last_tile_layout = None;
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
        self.update_window_title();
    }

    /// Swap tabs at positions `a` and `b`, keeping the active tab index pointing
    /// to the same content, and updating the MRU order and custom names.
    pub(crate) fn swap_tabs(&mut self, a: usize, b: usize) {
        if a == b || a >= self.tabs.len() || b >= self.tabs.len() {
            return;
        }
        self.tabs.swap(a, b);
        if self.active_tab == a {
            self.active_tab = b;
        } else if self.active_tab == b {
            self.active_tab = a;
        }
        for idx in &mut self.tab_mru {
            if *idx == a {
                *idx = b;
            } else if *idx == b {
                *idx = a;
            }
        }
        let a_name = self.tab_names.remove(&a);
        let b_name = self.tab_names.remove(&b);
        if let Some(n) = a_name {
            self.tab_names.insert(b, n);
        }
        if let Some(n) = b_name {
            self.tab_names.insert(a, n);
        }
        self.last_tile_layout = None;
        self.dirty = true;
    }

    pub(crate) fn reap_dead_panes(&mut self) {
        let dead: Vec<PaneId> = self
            .panes
            .iter_mut()
            .filter_map(|(id, pane)| if pane.is_alive() { None } else { Some(*id) })
            .collect();
        for id in dead {
            let Some(tab_idx) = self.tabs.iter().position(|t| t.panes().contains(&id)) else {
                continue;
            };
            if self.tabs[tab_idx].panes().len() > 1 {
                self.close_pane_in_any_tab(id);
            } else {
                // The last pane of a tab closes the whole tab (or, if it is
                // also the last tab, requests app exit) via `close_tab`.
                self.close_tab(tab_idx);
            }
        }
    }

    pub(crate) fn drain_all_panes(&mut self) -> bool {
        let mut any = false;
        let mut new_entries: Vec<(PaneId, crate::terminal::block_queue::BlockEntry)> = Vec::new();
        let mut patched_tiles: Vec<(PaneId, usize)> = Vec::new();
        let mut new_titles: Vec<(PaneId, String)> = Vec::new();
        // OSC 52 writes from the PTY, applied after the loop so the shared
        // clipboard handle can be borrowed without conflicting with `iter_mut`.
        let mut clipboard_write: Option<String> = None;
        // Panes whose PTY raised an OSC 52 read query, answered after the loop.
        let mut clipboard_reads: Vec<PaneId> = Vec::new();
        // Re-assert the block trust ceiling every pump rather than at pane
        // construction: panes are created from a dozen places (new tab, split,
        // session restore, mux attach), and a construction site that forgot to
        // apply the policy would silently over-grant. Setting it here costs a
        // field write per pane and cannot be missed.
        let max_trust = self.config.security.block_max_trust;
        for (_, pane) in self.panes.iter_mut() {
            pane.block_queue_mut().set_max_trust(max_trust);
        }
        for (id, pane) in self.panes.iter_mut() {
            let prev_count = pane.block_queue().entries().len();
            if pane.drain_output() {
                pane.grid_mut().detect_urls();
                any = true;
            }
            if let Some(text) = pane.take_clipboard_write() {
                clipboard_write = Some(text);
            }
            if pane.take_clipboard_read() {
                clipboard_reads.push(*id);
            }
            // Drain the terminal bell flag; the tab notification indicator was
            // removed, so the bell no longer drives any UI.
            pane.take_bell();
            if let Some(title) = pane.take_title() {
                new_titles.push((*id, title));
            }
            let curr_entries = pane.block_queue().entries();
            if curr_entries.len() > prev_count {
                for entry in &curr_entries[prev_count..] {
                    new_entries.push((*id, entry.clone()));
                }
            }
            let patched = pane.drain_live_patches();
            for idx in patched {
                patched_tiles.push((*id, idx));
            }
        }
        if !new_titles.is_empty() {
            for (id, title) in new_titles {
                self.pane_titles.insert(id, title);
            }
            self.update_window_title();
        }
        if !new_entries.is_empty() {
            self.create_block_tiles(&new_entries);
        }
        if !patched_tiles.is_empty() {
            self.update_live_tiles(&patched_tiles);
        }
        if let Some(text) = clipboard_write {
            if let Some(cb) = self.clipboard() {
                let _ = cb.set_text(&text);
            }
        }
        // OSC 52 reads answer only when `clipboard-read` opted in: the query
        // is silent on the tool's side, so the default must stay a refusal.
        if !clipboard_reads.is_empty() && self.config.clipboard_read {
            let text = self
                .clipboard()
                .and_then(|cb| cb.get_text().ok())
                .unwrap_or_default();
            let response = crate::terminal::pane::osc52_read_response(&text);
            for id in clipboard_reads {
                if let Some(pane) = self.panes.get_mut(&id) {
                    pane.write(&response);
                }
            }
        }
        any
    }

    pub(crate) fn resize_all_panes(&mut self) {
        let (cw, ch) = if let Some(renderer) = &self.renderer {
            renderer.cell_size()
        } else {
            (9.0, 20.0)
        };

        let layout_vp = self.content_viewport();
        let rects = self.tab().rects(layout_vp);

        // Size each grid to the renderer's content area (it insets every pane by
        // PANE_H_PAD horizontally). Computing cols from the raw rect width would
        // make the grid a column wider than what is drawn, pushing the scrollbar
        // past the pane's right edge (where the rightmost pane's bar gets clipped
        // by the surface, looking thinner than the others). Sizes are collected
        // first so the renderer borrow does not overlap the `panes` mutation.
        let sizes: Vec<(PaneId, usize, usize)> = rects
            .iter()
            .map(|(id, rect)| {
                let (cols, rows) = match &self.renderer {
                    Some(renderer) => renderer.grid_size_for(Self::layout_rect_to_pane(*rect)),
                    None => (
                        (rect.width / cw).floor().max(1.0) as usize,
                        (rect.height / ch).floor().max(1.0) as usize,
                    ),
                };
                (*id, cols, rows)
            })
            .collect();

        for (id, cols, rows) in sizes {
            if let Some(pane) = self.panes.get_mut(&id) {
                // A resize reflows the grid, which snaps the view back to the live
                // bottom; put the pane back where the user was reading. Starting or
                // ending a `/` search toggles the forced status bar, resizing every
                // pane by a row — losing the scroll position there would yank the
                // viewport away from the match being browsed.
                let offset = pane.grid().scroll_offset();
                pane.resize(cols.max(1), rows.max(1));
                if offset > 0 {
                    pane.grid_mut().set_scroll_offset(offset);
                }
            }
        }
    }

    /// Rounds the window size to the nearest whole row/column fit so a drag
    /// settles with zero leftover slack. Skipped while maximized/fullscreen;
    /// a no-op once already snapped, so `Resized` can't loop.
    pub(crate) fn snap_window_to_cell_grid(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if window.is_maximized() || window.fullscreen().is_some() {
            return;
        }
        let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) else {
            return;
        };
        if cw <= 0.0 || ch <= 0.0 {
            return;
        }
        let size = window.inner_size();
        let top_h_on_screen = if self.config.menu_style == MenuStyle::Modern {
            winter_render::modern_tabbar_height_px(ch)
        } else {
            self.top_chrome_rows() as f32 * ch
        };
        let status_h = if self.status_bar_visible() {
            winter_render::STATUS_BAR_HEIGHT * ch
        } else {
            0.0
        };
        let ideal_h = snap_height_to_rows(size.height as f32, top_h_on_screen, status_h, ch) as u32;
        let ideal_w =
            snap_width_to_cols(size.width as f32, 2.0 * winter_render::PANE_H_PAD, cw) as u32;
        if ideal_h == size.height && ideal_w == size.width {
            return;
        }
        let applied = window.request_inner_size(winit::dpi::PhysicalSize::new(ideal_w, ideal_h));
        // Some platforms apply the requested size synchronously and never send
        // the follow-up `Resized` that would otherwise drive this; without
        // this, the GPU surface and pane grids are left at the old size while
        // the OS-reported window is already the new one, showing as a gap.
        if let Some(actual) = applied {
            let scale_factor = window.scale_factor();
            if let Some(renderer) = &mut self.renderer {
                renderer.resize(actual.width, actual.height, scale_factor);
            }
            self.resize_all_panes();
        }
    }

    fn handle_palette_input(
        &mut self,
        palette: &mut Palette,
        key: &Key,
        physical: &PhysicalKey,
        focused: PaneId,
    ) {
        // Prompt undo/redo chords (default `Ctrl-/` / `Ctrl-\`) also drive the
        // palette query history, resolved through the configurable keymap.
        let mods = self.modifiers.state();
        let model_key = input::Key {
            alt: mods.alt_key(),
            code: winit_key_to_code(key, physical),
            ctrl: mods.control_key(),
            shift: mods.shift_key(),
        };
        match self.window_keymap.edit_binding(&model_key) {
            Some(EditBinding::Undo) => {
                palette.undo();
                return;
            }
            Some(EditBinding::Redo) => {
                palette.redo();
                return;
            }
            _ => {}
        }
        if mods.control_key() {
            if let Key::Character(c) = key.as_ref() {
                let lower = c.to_lowercase();
                if lower == "n" {
                    palette.move_down();
                    if palette.mode == PaletteMode::Swoop {
                        self.update_swoop_preview(palette, focused);
                    }
                    return;
                } else if lower == "p" {
                    palette.move_up();
                    if palette.mode == PaletteMode::Swoop {
                        self.update_swoop_preview(palette, focused);
                    }
                    return;
                }
            }
        }
        if mods.alt_key() {
            if let Key::Character(c) = key.as_ref() {
                let lower = c.to_lowercase();
                if lower == "p" {
                    palette.history_prev();
                    if palette.mode == PaletteMode::Swoop {
                        self.update_swoop_preview(palette, focused);
                    }
                    return;
                } else if lower == "n" {
                    palette.history_next();
                    if palette.mode == PaletteMode::Swoop {
                        self.update_swoop_preview(palette, focused);
                    }
                    return;
                }
            }
        }
        match key {
            Key::Named(NamedKey::Escape) => {
                if palette.mode == PaletteMode::Swoop {
                    if let Some((pid, (r, c))) = self.swoop_initial_cursor.take() {
                        if pid == focused {
                            self.nav_cursors.insert(focused, (r, c));
                            self.reveal_position(focused, (r, c));
                        }
                    }
                }
                palette.close();
                self.palette = None;
                self.dirty = true;
                return;
            }
            Key::Named(NamedKey::Enter) => {
                self.confirm_palette_selection(palette, focused);
                palette.close();
                self.palette = None;
                self.dirty = true;
                return;
            }
            Key::Named(NamedKey::Backspace) => {
                palette.pop_char();
            }
            Key::Named(NamedKey::ArrowUp) => {
                palette.move_up();
            }
            Key::Named(NamedKey::ArrowDown) => {
                palette.move_down();
            }
            Key::Character(c) => {
                // In the pane switcher, the digit shown next to an entry
                // (e.g. "2") jumps straight to it instead of filtering.
                let shortcut_hit = (palette.mode == PaletteMode::Panes)
                    .then(|| palette.position_by_shortcut(c))
                    .flatten();
                if let Some(pos) = shortcut_hit {
                    palette.selected = pos;
                    self.confirm_palette_selection(palette, focused);
                    palette.close();
                    self.palette = None;
                    self.dirty = true;
                    return;
                } else {
                    for ch in c.chars() {
                        palette.push_char(ch);
                    }
                }
            }
            _ => {}
        }
        if palette.mode == PaletteMode::Swoop {
            self.update_swoop_preview(palette, focused);
        }
    }

    /// Act on the palette's currently selected entry (`Enter`, or a pane
    /// switcher digit-jump): run a command, replay shell history, `cd` into a
    /// recent directory, or switch to a pane, depending on `palette.mode`.
    fn confirm_palette_selection(&mut self, palette: &Palette, focused: PaneId) {
        self.record_palette_query(&palette.query);
        let action = palette.selected_action().map(str::to_string);
        match palette.mode {
            PaletteMode::Commands => {
                if let Some(action) = action {
                    self.run_command(&action, focused);
                }
            }
            PaletteMode::History => {
                if let Some(cmd) = action {
                    if let Some(pane) = self.panes.get_mut(&focused) {
                        pane.write(cmd.as_bytes());
                    }
                }
            }
            PaletteMode::RecentDirs => {
                if let Some(dir) = action {
                    // Reject paths containing control characters — a
                    // malicious OSC 7 sequence could embed a newline to
                    // inject a second shell command.
                    let safe = !dir.chars().any(|c| c.is_control());
                    if safe {
                        if let Some(pane) = self.panes.get_mut(&focused) {
                            // Single-quote the path so shell metacharacters
                            // in the directory name are inert. The only
                            // character that cannot appear inside single
                            // quotes is `'` itself, escaped as `'\''`.
                            let escaped = dir.replace('\'', "'\\''");
                            let cmd = format!("cd '{}'\n", escaped);
                            pane.write(cmd.as_bytes());
                        }
                    }
                }
            }
            PaletteMode::Panes => {
                if let Some(pane_id_str) = action {
                    if let Ok(pane_id_val) = pane_id_str.parse::<u64>() {
                        self.switch_to_pane(PaneId(pane_id_val));
                    }
                }
            }
            PaletteMode::Swoop => {
                if let Some(action) = action {
                    if let Ok(abs_row) = action.parse::<usize>() {
                        if let Some((_pid, origin)) = self.swoop_initial_cursor.take() {
                            self.jump_lists.entry(focused).or_default().push(origin);
                        }
                        self.nav_cursors.insert(focused, (abs_row, 0));
                        self.reveal_position(focused, (abs_row, 0));
                        self.modes.insert(focused, Mode::Normal);
                    }
                }
            }
            PaletteMode::MuxSessions => {
                if let Some(session_entry) = action {
                    let session_name = session_entry
                        .split_whitespace()
                        .next()
                        .unwrap_or(&session_entry);
                    if !session_name.starts_with('(') {
                        self.new_mux_tab(session_name);
                    }
                }
            }
            PaletteMode::MuxKill => {
                if let Some(session_entry) = action {
                    let session_name = session_entry
                        .split_whitespace()
                        .next()
                        .unwrap_or(&session_entry);
                    if !session_name.starts_with('(') {
                        let sock_path = crate::mux::server::default_socket_path();
                        if let Ok(mut client) = crate::mux::client::MuxClient::connect(&sock_path) {
                            let _ = client.kill(session_name);
                            self.set_notice(format!("killed mux session '{session_name}'"));
                        } else {
                            self.set_error("could not connect to mux daemon");
                        }
                    }
                }
            }
            PaletteMode::MuxNew => {
                // The query is the input: "name [command...]".
                match parse_mux_spawn_query(&palette.query) {
                    Some((name, command)) => {
                        // Start where the user is: the focused pane's OSC 7
                        // cwd, when the shell has reported one.
                        let cwd = self.panes.get(&focused).and_then(|pane| pane.cwd());
                        self.spawn_mux_tab_at(
                            &crate::mux::server::default_socket_path(),
                            &name,
                            command.as_deref(),
                            cwd.as_deref(),
                            focused,
                        );
                    }
                    None => self.set_error("mux new: usage: name [command]"),
                }
            }
            PaletteMode::MuxAttachRemote => {
                // The query is the input: "host [session]".
                match parse_mux_attach_query(&palette.query) {
                    Some((host, session)) => {
                        self.new_mux_tab_remote_at(&host, session.as_deref().unwrap_or("default"));
                    }
                    None => self.set_error("mux attach: usage: host [session]"),
                }
            }
        }
    }

    fn record_palette_query(&mut self, query: &str) {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return;
        }
        self.palette_history.retain(|q| q != trimmed);
        self.palette_history.insert(0, trimmed.to_string());
        if self.palette_history.len() > 100 {
            self.palette_history.truncate(100);
        }
        self.save_app_state();
    }

    fn save_app_state(&self) {
        let window_size = self.window.as_ref().map(|w| {
            let s = w.inner_size();
            (s.width, s.height)
        });
        crate::config::save_state(&crate::config::AppState {
            palette_history: self.palette_history.clone(),
            window_size,
        });
    }

    /// Run a named command, shared by the command palette and the menus.
    pub(crate) fn run_command(&mut self, action: &str, focused: PaneId) {
        match action {
            "new_tab" => {
                self.new_tab();
            }
            "close_tab" => {
                self.close_tab(self.active_tab);
            }
            "rename_tab" => {
                self.tab_rename_input = Some(
                    self.tab_names
                        .get(&self.active_tab)
                        .cloned()
                        .unwrap_or_default(),
                );
            }
            "next_tab" => {
                self.cycle_tab(true);
            }
            "prev_tab" => {
                self.cycle_tab(false);
            }
            "recent_tab_back" => {
                self.recent_tab(false);
            }
            "recent_tab_forward" => {
                self.recent_tab(true);
            }
            "reload" => {
                self.pending_reload = true;
            }
            "toggle_mode" => {
                let mode = self.modes.get(&focused).copied().unwrap_or_default();
                let new_mode = match mode {
                    Mode::Insert => Mode::Normal,
                    Mode::Normal | Mode::Visual | Mode::BlockFocus => Mode::Insert,
                };
                self.modes.insert(focused, new_mode);
            }
            "split_horizontal" => {
                self.split_pane(Direction::Horizontal);
            }
            "split_vertical" => {
                self.split_pane(Direction::Vertical);
            }
            "close_pane" => {
                self.close_pane(focused);
            }
            "copy_cwd" => {
                self.copy_pane_cwd(focused);
            }
            "focus_down" | "focus_up" | "focus_left" | "focus_right" => {
                let dir = match action {
                    "focus_down" => FocusDir::Down,
                    "focus_up" => FocusDir::Up,
                    "focus_left" => FocusDir::Left,
                    _ => FocusDir::Right,
                };
                let viewport = self.viewport_rect();
                let layout_vp = Rect::new(viewport.x, viewport.y, viewport.width, viewport.height);
                self.tab_mut().focus_in_direction(dir, layout_vp);
            }
            "search" => {
                self.search_query = Some(String::new());
            }
            "next_block" => {
                self.focus_block(input::BlockNav::Next, focused);
            }
            "prev_block" => {
                self.focus_block(input::BlockNav::Previous, focused);
            }
            "quick_select" => {
                self.enter_quick_select(focused);
            }
            "yank_block" => {
                self.yank_block_source(focused);
            }
            "toggle_fold" => {
                let folded = self.folded_blocks.entry(focused).or_default();
                if folded.is_empty() {
                    folded.insert(0);
                } else {
                    folded.clear();
                }
                self.dirty = true;
            }
            "theme_dark" => {
                self.config.theme = crate::config::ThemeSetting::Dark;
                self.rebuild_theme();
            }
            "theme_light" => {
                self.config.theme = crate::config::ThemeSetting::Light;
                self.rebuild_theme();
            }
            "theme_auto" => {
                self.config.theme = crate::config::ThemeSetting::Auto;
                self.rebuild_theme();
            }
            "theme_new" => {
                self.theme_name_input = Some(String::new());
            }
            "open_settings" => {
                self.open_settings();
            }
            "toggle_pane_zoom" => {
                self.tab_mut().toggle_zoom();
                if self.renderer.is_some() {
                    self.resize_all_panes();
                }
                self.dirty = true;
            }
            "cd_recent" => {
                let dirs = self
                    .panes
                    .get(&focused)
                    .map(|p| {
                        let mut seen = std::collections::HashSet::new();
                        p.scrollback()
                            .blocks()
                            .iter()
                            .rev()
                            .filter_map(|b| b.cwd.as_deref())
                            .filter(|&d| seen.insert(d.to_string()))
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                self.palette = Some(
                    Palette::open_recent_dirs(dirs)
                        .with_query_history(self.palette_history.clone()),
                );
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            "select_pane" => {
                let mut panes_list = Vec::new();
                for (tab_index, tab) in self.tabs.iter().enumerate() {
                    // Mirror the tab-bar format on the left ("<tab number>:
                    // <title>"); the pane number goes on the right.
                    let label = format!("{}: {}", tab_index + 1, self.tab_title(tab_index));
                    for (pane_index, &pane_id) in tab.panes().iter().enumerate() {
                        let shortcut = (pane_index + 1).to_string();
                        panes_list.push((pane_id, label.clone(), shortcut));
                    }
                }
                self.palette = Some(
                    Palette::open_panes(panes_list)
                        .with_query_history(self.palette_history.clone()),
                );
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            "swoop" => {
                self.open_swoop(focused);
            }
            "copy_scrollback" => {
                self.copy_scrollback(focused);
            }
            "export_scrollback_ansi" => {
                self.export_scrollback_ansi(focused);
            }
            "export_scrollback_editor" => {
                self.export_scrollback_editor(focused);
            }
            "export_scrollback_html" => {
                self.export_scrollback_html(focused);
            }
            "export_block_text" => {
                self.export_focused_block_text(focused);
            }
            "export_block_svg" => {
                self.export_focused_block_svg(focused);
            }
            "toggle_rainbow_parens" => {
                self.toggle_rainbow_parens();
            }
            "toggle_sentence_highlight" => {
                self.toggle_sentence_highlight();
            }
            "mux_list_sessions" => {
                self.open_mux_palette();
            }
            "mux_new_session" => {
                self.palette =
                    Some(Palette::open_mux_new().with_query_history(self.palette_history.clone()));
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            "mux_attach_remote" => {
                self.palette = Some(
                    Palette::open_mux_attach_remote()
                        .with_query_history(self.palette_history.clone()),
                );
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            "mux_kill_session" => {
                self.open_mux_kill_palette();
            }
            "mux_detach_session" => {
                let session = self
                    .panes
                    .get(&focused)
                    .and_then(|pane| pane.mux_session().map(str::to_string));
                match session {
                    Some(name) => {
                        if self.tabs.len() <= 1 && self.tabs[0].panes().len() <= 1 {
                            self.set_notice(
                                "cannot detach the only pane — close the window or open another tab",
                            );
                        } else {
                            self.set_notice(format!("detached from mux session '{name}'"));
                            // Closing the pane drops the mux client, which
                            // detaches server-side; the session keeps running.
                            self.close_pane(focused);
                        }
                    }
                    None => self.set_notice("this pane is not attached to a mux session"),
                }
            }
            // Palette entries that mirror a keybindable window command
            // (`copy_selection`, `font_increase`, scrolling, …) dispatch
            // through the same mapping their chords use, so selecting one
            // from the palette and pressing its keybinding are equivalent.
            other => {
                if let Some(action) = crate::model::input::window_action_by_name(other) {
                    self.handle_action(action, focused);
                }
            }
        }
    }

    /// Query the mux server's session list for a palette, formatted as
    /// `name (colsxrows, up UPTIME, N attached) - command` entries whose
    /// first word is the session name; placeholder entries start with `(`
    /// so the palette's confirm handler skips them. `empty_fallback` is
    /// offered when the server holds no sessions, since attaching to a
    /// missing session creates it.
    fn mux_session_entries(&self, empty_fallback: &str) -> Vec<String> {
        let sock_path = crate::mux::server::default_socket_path();
        let mut client = match crate::mux::client::MuxClient::connect(&sock_path) {
            Ok(client) => client,
            Err(_) => {
                return vec!["(no daemon running — start with 'winter mux serve')".to_string()]
            }
        };
        // The server answers on its own poll cycle, so the query polls with
        // a deadline rather than racing it with one nonblocking read (which
        // always saw nothing and fell back to placeholder data). The wait
        // is bounded: this runs on the UI thread while the palette opens.
        match client.query_sessions(std::time::Duration::from_millis(200)) {
            Ok(sessions) if sessions.is_empty() => vec![empty_fallback.to_string()],
            Ok(sessions) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                sessions
                    .iter()
                    .map(|s| {
                        let uptime = crate::mux::protocol::format_uptime(s.created, now);
                        format!(
                            "{} ({}x{}, up {uptime}, {} attached) - {}",
                            s.name, s.cols, s.rows, s.attach_count, s.command
                        )
                    })
                    .collect()
            }
            Err(_) => vec!["(could not query sessions)".to_string()],
        }
    }

    /// Open the command palette to list or attach running background mux sessions.
    pub(crate) fn open_mux_palette(&mut self) {
        let sessions = self.mux_session_entries("default (80x24)");
        self.palette = Some(
            Palette::open_mux_sessions(sessions).with_query_history(self.palette_history.clone()),
        );
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Open the command palette to select a background mux session to kill.
    pub(crate) fn open_mux_kill_palette(&mut self) {
        let sessions = self.mux_session_entries("(no running sessions)");
        self.palette =
            Some(Palette::open_mux_kill(sessions).with_query_history(self.palette_history.clone()));
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Spawn a named session on the mux server at `path` — running
    /// `command` (or the default shell) in `cwd` — then open a tab attached
    /// to it. The spawn is confirmed with a bounded wait (the server answers
    /// on its poll cycle) before attaching, so failures surface as notices
    /// instead of a tab pointing at a session that never existed.
    fn spawn_mux_tab_at(
        &mut self,
        path: &str,
        name: &str,
        command: Option<&str>,
        cwd: Option<&str>,
        focused: PaneId,
    ) {
        let mut client = match crate::mux::client::MuxClient::connect(path) {
            Ok(client) => client,
            Err(_) => {
                self.set_error("could not connect to mux daemon");
                return;
            }
        };
        // Size the session to the focused pane so the first frame fits.
        let (cols, rows) = self
            .panes
            .get(&focused)
            .map(|pane| {
                (
                    pane.grid().cols().max(1) as u16,
                    pane.grid().rows().max(1) as u16,
                )
            })
            .unwrap_or((80, 24));
        match client.spawn_confirmed(
            name,
            cols,
            rows,
            cwd,
            command,
            std::time::Duration::from_millis(200),
        ) {
            Ok((cols, rows)) => {
                self.new_mux_tab_at(path, name);
                let note = match command {
                    Some(command) => format!("started '{name}' ({cols}x{rows}): {command}"),
                    None => format!("started '{name}' ({cols}x{rows})"),
                };
                self.set_notice(note);
            }
            Err(message) => self.set_error(format!("mux: {message}")),
        }
    }

    /// Buffer swoop: open fuzzy line search over the focused pane's grid and scrollback.
    pub(crate) fn open_swoop(&mut self, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let initial = self.nav_cursor(focused).or_else(|| {
            Some((
                pane.grid().to_absolute_row(pane.grid().cursor().0),
                pane.grid().cursor().1,
            ))
        });
        if let Some(pos) = initial {
            self.swoop_initial_cursor = Some((focused, pos));
        }
        self.modes.insert(focused, Mode::Normal);
        let lines = navigation::swoop::extract_swoop_lines(pane.grid());
        let palette = Palette::open_swoop(lines).with_query_history(self.palette_history.clone());
        if let Some(action) = palette.selected_action() {
            if let Ok(abs_row) = action.parse::<usize>() {
                self.nav_cursors.insert(focused, (abs_row, 0));
                self.reveal_position(focused, (abs_row, 0));
            }
        }
        self.palette = Some(palette);
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Update the live preview cursor during Buffer Swoop.
    pub(crate) fn update_swoop_preview(&mut self, palette: &Palette, focused: PaneId) {
        if let Some(action) = palette.selected_action() {
            if let Ok(abs_row) = action.parse::<usize>() {
                self.nav_cursors.insert(focused, (abs_row, 0));
                self.reveal_position(focused, (abs_row, 0));
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }
    }

    /// Copy the focused pane's full scrollback and visible text to clipboard.
    pub(crate) fn copy_scrollback(&mut self, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let text = navigation::export::export_scrollback_plain(pane.grid());
        let copied = self
            .clipboard
            .as_mut()
            .and_then(|cb| cb.set_text(text).ok())
            .is_some();
        if copied {
            self.set_notice("copied scrollback to clipboard");
        } else {
            self.set_error("could not copy to clipboard");
        }
    }

    /// Export scrollback with ANSI color escape codes to a file and open it.
    pub(crate) fn export_scrollback_ansi(&mut self, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let theme = self
            .renderer
            .as_ref()
            .map(|r| r.theme())
            .cloned()
            .unwrap_or_default();
        let ansi = navigation::export::export_scrollback_ansi(pane.grid(), &theme);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("winter-scrollback-ansi-{timestamp}.txt"));
        if let Err(e) = std::fs::write(&path, ansi) {
            self.set_error(format!("could not write ANSI scrollback file: {e}"));
            return;
        }
        self.open_file_in_new_tab(path, None);
        self.set_notice("opened ANSI scrollback in editor");
    }

    /// Export scrollback and open in a new tab running $VISUAL / $EDITOR.
    pub(crate) fn export_scrollback_editor(&mut self, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let text = navigation::export::export_scrollback_plain(pane.grid());
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("winter-scrollback-{timestamp}.txt"));
        if let Err(e) = std::fs::write(&path, text) {
            self.set_error(format!("could not write scrollback file: {e}"));
            return;
        }
        self.open_file_in_new_tab(path, None);
        self.set_notice("opened scrollback in editor");
    }

    /// Export scrollback as HTML and open it in the default web browser.
    pub(crate) fn export_scrollback_html(&mut self, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let theme = self
            .renderer
            .as_ref()
            .map(|r| r.theme())
            .cloned()
            .unwrap_or_default();
        let html = navigation::export::export_scrollback_html(pane.grid(), &theme);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("winter-scrollback-{timestamp}.html"));
        if let Err(e) = std::fs::write(&path, html) {
            self.set_error(format!("could not write scrollback HTML: {e}"));
            return;
        }
        match ::open::that(&path) {
            Ok(()) => self.set_notice(format!("opened {}", path.display())),
            Err(e) => self.set_error(format!("could not open HTML file: {e}")),
        }
    }

    pub(crate) fn toggle_rainbow_parens(&mut self) {
        self.config.rainbow_parens = !self.config.rainbow_parens;
        let state = if self.config.rainbow_parens {
            "enabled"
        } else {
            "disabled"
        };
        self.set_notice(format!("rainbow parens {state}"));
        self.dirty = true;
    }

    pub(crate) fn toggle_sentence_highlight(&mut self) {
        self.config.sentence_highlight = !self.config.sentence_highlight;
        let state = if self.config.sentence_highlight {
            "enabled"
        } else {
            "disabled"
        };
        self.set_notice(format!("sentence highlight {state}"));
        self.dirty = true;
    }

    /// Rebuild the renderer theme from the current `config.theme` selection plus
    /// any color overrides, and request a redraw. Shared by the theme menu
    /// commands and the live settings page.
    pub(crate) fn rebuild_theme(&mut self) {
        use crate::config::ThemeSetting;
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let mut theme = match &self.config.theme {
            ThemeSetting::Dark => Theme::dark(),
            ThemeSetting::Light => Theme::light(),
            ThemeSetting::Auto => self
                .window
                .as_ref()
                .and_then(|w| w.theme())
                .map(|t| match t {
                    winit::window::Theme::Dark => Theme::dark(),
                    winit::window::Theme::Light => Theme::light(),
                })
                .unwrap_or_default(),
            // A user theme file; fall back to the dark preset if it is missing.
            ThemeSetting::Named(name) => {
                crate::config::load_named_theme(name).unwrap_or_else(Theme::dark)
            }
        };
        self.config.colors.apply(&mut theme);
        renderer.set_theme(theme);
        self.dirty = true;
    }

    /// Validate `name`, save the currently active (resolved) theme's colors as
    /// `themes/<name>.kdl`, and switch to it. Reports success or failure as a
    /// status notice. Like the `theme_dark`/`theme_light`/`theme_auto` quick
    /// commands, this only previews the switch in `config.theme`; it does not
    /// persist the selection to `settings.kdl`.
    fn create_named_theme(&mut self, name: &str) {
        let name = name.trim();
        if !crate::config::is_valid_theme_name(name) {
            self.set_error("Theme name must use only letters, numbers, - or _");
            return;
        }
        let theme = self
            .renderer
            .as_ref()
            .map_or_else(Theme::default, |r| r.theme().clone());
        match crate::config::save_named_theme(name, &theme) {
            Ok(()) => {
                self.config.theme = crate::config::ThemeSetting::Named(name.to_string());
                self.rebuild_theme();
                self.set_notice(format!("Created theme \"{name}\""));
            }
            Err(e) => self.set_error(e.to_string()),
        }
    }

    /// Open the full-window settings page, dismissing any open menu or palette
    /// first. A no-op if it is already open.
    pub(crate) fn open_settings(&mut self) {
        if self.settings_page.is_some() {
            return;
        }
        self.close_menu();
        self.palette = None;
        self.settings_page = Some(self.build_settings_page());
        // The page covers the window; hide block tiles so they don't show over it.
        self.webview_mgr.hide_all();
        self.dirty = true;
    }

    /// Side effects of leaving the settings page: re-show block tiles and redraw.
    fn on_settings_closed(&mut self) {
        // The overlay is gone; force block tiles to re-show and re-position.
        self.last_tile_layout = None;
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Build the settings page from the live config: one row per editable setting,
    /// pre-filled with the current value.
    fn build_settings_page(&self) -> SettingsPage {
        let theme_options = settings_theme_options();
        let theme_value = self.config.theme.as_value();
        let theme_index = theme_options
            .iter()
            .position(|o| o.value == theme_value)
            .unwrap_or(0);

        let menu_options = vec![
            ChoiceOption {
                label: "Modern".into(),
                value: "modern".into(),
            },
            ChoiceOption {
                label: "Classic".into(),
                value: "classic".into(),
            },
        ];
        let menu_index = match self.config.menu_style {
            MenuStyle::Modern => 0,
            MenuStyle::Classic => 1,
        };

        let status = &self.config.status_bar;
        let fields = vec![
            SettingsField::choice("theme", "Theme", theme_options, theme_index)
                .in_section("Appearance")
                .with_note("Color palette for the terminal and chrome. To add a custom one, run \"Theme: Create New...\" from the command palette"),
            SettingsField::choice("menu_style", "Menu style", menu_options, menu_index)
                .with_note("Modern hamburger menu or a classic menubar"),
            SettingsField::toggle("status.enabled", "Show status bar", status.enabled)
                .in_section("Status bar"),
            SettingsField::toggle("status.show_mode", "Show mode indicator", status.show_mode),
            SettingsField::text(
                "font_family",
                "Font family",
                self.config.font_family.clone().unwrap_or_default(),
            )
            .in_section("Text")
            .with_note("Applied on restart"),
            SettingsField::number(
                "font_size",
                "Font size",
                self.config.font_size,
                MIN_FONT_SIZE,
                MAX_FONT_SIZE,
                FONT_SIZE_STEP,
                0,
            )
            .with_note("Applied on restart"),
            SettingsField::text(
                "font_weight",
                "Font weight",
                self.config.font_weight.clone().unwrap_or_default(),
            )
            .in_section("Text")
            .with_note("e.g. 300, light, normal. Applied on restart"),
            SettingsField::text(
                "font_weight_bold",
                "Bold font weight",
                self.config.font_weight_bold.clone().unwrap_or_default(),
            )
            .in_section("Text")
            .with_note("e.g. 500, bold, medium. Applied on restart"),
            SettingsField::number(
                "opacity",
                "Opacity",
                self.config.opacity,
                MIN_OPACITY,
                MAX_OPACITY,
                OPACITY_STEP,
                2,
            )
            .with_note("Applied on restart"),
            {
                let cursor_options = vec![
                    ChoiceOption { label: "Block".into(), value: "block".into() },
                    ChoiceOption { label: "Bar".into(), value: "bar".into() },
                    ChoiceOption { label: "Underline".into(), value: "underline".into() },
                ];
                let idx = ["block", "bar", "underline"]
                    .iter()
                    .position(|&v| v == self.config.cursor.insert.as_value())
                    .unwrap_or(1);
                SettingsField::choice("cursor.insert", "Cursor (insert)", cursor_options, idx)
                    .in_section("Cursor")
            },
            {
                let cursor_options = vec![
                    ChoiceOption { label: "Block".into(), value: "block".into() },
                    ChoiceOption { label: "Bar".into(), value: "bar".into() },
                    ChoiceOption { label: "Underline".into(), value: "underline".into() },
                ];
                let idx = ["block", "bar", "underline"]
                    .iter()
                    .position(|&v| v == self.config.cursor.normal.as_value())
                    .unwrap_or(0);
                SettingsField::choice("cursor.normal", "Cursor (normal)", cursor_options, idx)
            },
            {
                let cursor_options = vec![
                    ChoiceOption { label: "Block".into(), value: "block".into() },
                    ChoiceOption { label: "Bar".into(), value: "bar".into() },
                    ChoiceOption { label: "Underline".into(), value: "underline".into() },
                ];
                let idx = ["block", "bar", "underline"]
                    .iter()
                    .position(|&v| v == self.config.cursor.visual.as_value())
                    .unwrap_or(0);
                SettingsField::choice("cursor.visual", "Cursor (visual)", cursor_options, idx)
            },
            SettingsField::text(
                "shell",
                "Shell",
                self.config.active_shell().unwrap_or_default().to_string(),
            )
            .in_section("Terminal")
            .with_note("Shell for this OS. Saves to the per-OS key (shell-linux, shell-macos, shell-windows) in settings.kdl"),
            SettingsField::number(
                "scrollback_lines",
                "Scrollback lines",
                self.config.scrollback_lines.unwrap_or(winter_render::MAX_SCROLLBACK) as f32,
                MIN_SCROLLBACK,
                MAX_SCROLLBACK,
                SCROLLBACK_STEP,
                0,
            )
            .with_note("Applied to new panes"),
            SettingsField::toggle("rainbow_parens", "Rainbow Parentheses", self.config.rainbow_parens)
                .in_section("Terminal")
                .with_note("Depth-color matching bracket pairs and highlight unmatched closers"),
            SettingsField::toggle("sentence_highlight", "Sentence Highlight", self.config.sentence_highlight)
                .in_section("Terminal")
                .with_note("Alternating background tint per sentence for reading transcripts"),
            SettingsField::toggle("url_underline", "Underline URLs", self.config.url_underline)
                .in_section("Terminal")
                .with_note("Underline auto-detected and OSC 8 hyperlink URLs"),
            SettingsField::toggle("wrap_indent", "Hanging Indent", self.config.wrap_indent)
                .in_section("Terminal")
                .with_note("Indent soft-wrapped continuation lines to match the logical line's indent"),
            {
                let cursor_options = vec![
                    ChoiceOption { label: "Block".into(), value: "block".into() },
                    ChoiceOption { label: "Bar".into(), value: "bar".into() },
                    ChoiceOption { label: "Underline".into(), value: "underline".into() },
                ];
                let idx = ["block", "bar", "underline"]
                    .iter()
                    .position(|&v| v == self.config.cursor.block_focus.as_value())
                    .unwrap_or(1);
                SettingsField::choice("cursor.block_focus", "Cursor (block focus)", cursor_options, idx)
                    .in_section("Cursor")
            },
            SettingsField::toggle(
                "palette_match_underline",
                "Palette match underline",
                self.config.palette_match_underline,
            )
            .in_section("Palette")
            .with_note("Underline fuzzy-matched characters in palette results"),
            {
                let side_options = vec![
                    ChoiceOption { label: "Left".into(), value: "left".into() },
                    ChoiceOption { label: "Right".into(), value: "right".into() },
                ];
                let idx = if self.config.window_controls_side == ControlsSide::Left { 0 } else { 1 };
                SettingsField::choice("window_controls_side", "Window controls", side_options, idx)
                    .in_section("Window")
                    .with_note("Side for minimize/maximize/close buttons")
            },
            {
                let style_options = vec![
                    ChoiceOption { label: "Modern".into(), value: "modern".into() },
                    ChoiceOption { label: "System".into(), value: "system".into() },
                ];
                let idx = if self.config.title_bar_style == TitleBarStyle::Modern { 0 } else { 1 };
                SettingsField::choice("title_bar_style", "Title bar style", style_options, idx)
                    .with_note("Applied on restart")
            },
            SettingsField::text(
                "window_title_template",
                "Window title",
                self.config.window_title_template.clone(),
            )
            .in_section("Window")
            .with_note("Placeholders: {{ title }}, {{ app_name }}, {{ pane_title }}, {{ cwd }}. Empty resets"),
            SettingsField::toggle(
                "paste_on_right_click",
                "Paste on right-click",
                self.config.paste_on_right_click,
            )
            .in_section("Window")
            .with_note("Right-click pastes clipboard instead of opening the context menu"),
        ];
        SettingsPage::new(fields)
    }

    /// Route one key to the open settings page. Returns whether the page should
    /// stay open (`false` on Enter/Escape). Each value change is applied and
    /// persisted immediately, mirroring the WebView's live preview.
    fn handle_settings_input(&mut self, page: &mut SettingsPage, key: &Key) -> bool {
        match key {
            Key::Named(NamedKey::Escape) | Key::Named(NamedKey::Enter) => return false,
            Key::Named(NamedKey::ArrowUp) => page.move_up(),
            Key::Named(NamedKey::ArrowDown) => page.move_down(),
            Key::Named(NamedKey::ArrowLeft) => {
                if let Some((k, v)) = page.adjust(false) {
                    self.apply_settings_edit(&k, &v);
                }
            }
            Key::Named(NamedKey::ArrowRight) => {
                if let Some((k, v)) = page.adjust(true) {
                    self.apply_settings_edit(&k, &v);
                }
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some((k, v)) = page.pop_char() {
                    self.apply_settings_edit(&k, &v);
                }
            }
            // winit reports the space bar as a named key, not a character. On a
            // text row it inserts a space (font names have them); elsewhere it
            // flips the toggle or steps the control.
            Key::Named(NamedKey::Space) => {
                let edit = if page.selected_is_text() {
                    page.push_char(' ')
                } else {
                    page.adjust(true)
                };
                if let Some((k, v)) = edit {
                    self.apply_settings_edit(&k, &v);
                }
            }
            Key::Character(chars) => {
                for ch in chars.chars() {
                    if let Some((k, v)) = page.push_char(ch) {
                        self.apply_settings_edit(&k, &v);
                    }
                }
            }
            _ => {}
        }
        true
    }

    /// Apply one settings edit to the live config and persist it.
    fn apply_settings_edit(&mut self, key: &str, value: &str) {
        if self.apply_setting(key, value) {
            if let Err(e) = self.config.save() {
                eprintln!("winter: could not save settings: {e}");
            }
        }
        self.dirty = true;
    }

    /// Apply one settings edit to the live config and perform any renderer or
    /// layout refresh it implies. Returns whether the config changed (and so
    /// should be persisted); an unparseable value leaves the config untouched.
    fn apply_setting(&mut self, key: &str, value: &str) -> bool {
        use crate::config::ThemeSetting;
        match key {
            "theme" => {
                self.config.theme = ThemeSetting::from_value(value);
                self.rebuild_theme();
            }
            "menu_style" => {
                self.config.menu_style = match value {
                    "classic" => MenuStyle::Classic,
                    _ => MenuStyle::Modern,
                };
                self.relayout_tabbar();
            }
            "font_family" => {
                let trimmed = value.trim();
                self.config.font_family = (!trimmed.is_empty()).then(|| trimmed.to_string());
            }
            "font_weight" => {
                let trimmed = value.trim();
                self.config.font_weight = (!trimmed.is_empty()).then(|| trimmed.to_string());
            }
            "font_weight_bold" => {
                let trimmed = value.trim();
                self.config.font_weight_bold = (!trimmed.is_empty()).then(|| trimmed.to_string());
            }
            "font_size" => match value.parse::<f32>() {
                Ok(size) => self.config.font_size = size,
                Err(_) => return false,
            },
            "opacity" => match value.parse::<f32>() {
                Ok(opacity) => self.config.opacity = opacity.clamp(0.1, 1.0),
                Err(_) => return false,
            },
            "status.enabled" => {
                self.config.status_bar.enabled = value == "true";
                self.relayout_tabbar();
            }
            "status.show_mode" => {
                self.config.status_bar.show_mode = value == "true";
                self.dirty = true;
            }
            "cursor.insert" => {
                self.config.cursor.insert = CursorShape::from_value(value);
                self.dirty = true;
            }
            "cursor.normal" => {
                self.config.cursor.normal = CursorShape::from_value(value);
                self.dirty = true;
            }
            "cursor.visual" => {
                self.config.cursor.visual = CursorShape::from_value(value);
                self.dirty = true;
            }
            "shell" => {
                let trimmed = value.trim();
                let val = (!trimmed.is_empty()).then(|| trimmed.to_string());
                // Clear the generic `shell` so `to_kdl` doesn't emit both the
                // generic and the OS-specific key (which would be redundant).
                self.config.shell = None;
                #[cfg(target_os = "windows")]
                {
                    self.config.shell_windows = val;
                }
                #[cfg(target_os = "macos")]
                {
                    self.config.shell_macos = val;
                }
                #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
                {
                    self.config.shell_linux = val;
                }
            }
            "scrollback_lines" => match value.parse::<f32>() {
                Ok(n) if n >= 1.0 => {
                    self.config.scrollback_lines = Some(n as usize);
                }
                _ => return false,
            },
            "cursor.block_focus" => {
                self.config.cursor.block_focus = CursorShape::from_value(value);
                self.dirty = true;
            }
            "cursor.blink" => {
                self.config.cursor.blink = value == "true";
                if !self.config.cursor.blink {
                    self.blink_phase = true;
                }
                self.dirty = true;
            }
            "ligatures" => {
                self.config.ligatures = value == "true";
                if let Some(r) = &mut self.renderer {
                    r.set_ligatures(self.config.ligatures);
                }
                self.dirty = true;
            }
            "palette_match_underline" => {
                self.config.palette_match_underline = value == "true";
                self.dirty = true;
            }
            "rainbow_parens" => {
                self.config.rainbow_parens = value == "true";
                self.dirty = true;
            }
            "sentence_highlight" => {
                self.config.sentence_highlight = value == "true";
                self.dirty = true;
            }
            "url_underline" => {
                self.config.url_underline = value == "true";
                self.dirty = true;
            }
            "wrap_indent" => {
                let enabled = value == "true";
                self.config.wrap_indent = enabled;
                for pane in self.panes.values_mut() {
                    pane.grid_mut().set_wrap_indent(enabled);
                }
                self.dirty = true;
            }
            "window_controls_side" => {
                self.config.window_controls_side = match value {
                    "left" => ControlsSide::Left,
                    _ => ControlsSide::Right,
                };
                self.relayout_tabbar();
            }
            "title_bar_style" => {
                self.config.title_bar_style = TitleBarStyle::from_value(value);
            }
            "window_title_template" => {
                let trimmed = value.trim();
                self.config.window_title_template = if trimmed.is_empty() {
                    DEFAULT_WINDOW_TITLE_TEMPLATE.to_string()
                } else {
                    trimmed.to_string()
                };
                self.update_window_title();
            }
            "paste_on_right_click" => {
                self.config.paste_on_right_click = value == "true";
            }
            _ => return false,
        }
        true
    }

    /// Re-lay-out panes after a change to the reserved top-tabbar or status-bar
    /// rows (menu style, status-bar visibility), and request a redraw.
    fn relayout_tabbar(&mut self) {
        self.last_tile_layout = None;
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }

    /// Persist the session and exit the event loop. Shared by the native close
    /// request and the custom window-close control.
    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        if self.config.restore_session {
            Session::save(&self.tabs, self.active_tab, &self.panes);
        }
        self.panes.clear();
        event_loop.exit();
    }

    /// Persist the session, spawn a fresh instance of the current binary, and
    /// exit the event loop, so the binary can be replaced on disk while
    /// keeping the same tabs and panes across the restart. Unlike
    /// [`App::quit`], the session is always saved here regardless of
    /// `restore_session`, since reloading is an explicit request to carry
    /// state across the restart.
    fn reload(&mut self, event_loop: &ActiveEventLoop) {
        Session::save(&self.tabs, self.active_tab, &self.panes);
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).spawn();
        }
        self.panes.clear();
        event_loop.exit();
    }
}

// ========================================================================
// ApplicationHandler
// ========================================================================

// ========================================================================
// App: window-event handlers
// ========================================================================

/// The bodies of `window_event`'s largest match arms. They live here rather
/// than inline in the match so the dispatcher stays a dispatcher: it was 837
/// lines and 11 levels deep with these inlined.
impl App {
    /// Route one key press or release: the chrome overlays first (command
    /// palette, settings page, which-key), then the focused pane's modal
    /// keymap, and finally the PTY.
    fn on_keyboard_input(&mut self, event: KeyEvent, event_loop: &ActiveEventLoop) {
        // winit synthesizes KeyboardInput::Pressed events for every
        // physically-held key on XI_FocusIn (handle_pressed_keys). Swallow
        // those here so e.g. the Tab from Alt+Tab never reaches the PTY.
        if event.state == ElementState::Pressed && self.suppress_synthesized_keys {
            return;
        }

        // Windows-only: drop a key event that raced ahead of this
        // window's own focus-gain notification (see
        // `is_pre_focus_key_leak`'s doc for why this can happen).
        #[cfg(target_os = "windows")]
        if is_pre_focus_key_leak(
            event.state,
            self.window.as_ref().is_some_and(|w| w.has_focus()),
        ) {
            return;
        }

        let focused = self.tab().focused();
        let mods_state = self.modifiers.state();
        let code = winit_key_to_code(&event.logical_key, &event.physical_key);
        let key = input::Key {
            alt: mods_state.alt_key(),
            code,
            ctrl: mods_state.control_key(),
            shift: mods_state.shift_key(),
        };
        if event.state == ElementState::Released {
            let kitty_flags = self
                .panes
                .get(&focused)
                .map(|p| p.kitty_flags())
                .unwrap_or(0);
            let bytes = input::encode_release(&key, kitty_flags);
            if !bytes.is_empty() {
                if let Some(pane) = self.panes.get_mut(&focused) {
                    pane.write(&bytes);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            return;
        }

        // Reset blink to the "visible" phase on every key press so the
        // cursor is immediately shown as feedback for the action.
        if self.config.cursor.blink {
            self.blink_phase = true;
            self.blink_next_flip = Instant::now() + CURSOR_BLINK_PERIOD;
        }

        let mode = self.modes.get(&focused).copied().unwrap_or_default();

        // Taken (not just read) unconditionally so every key other than
        // the matching second Escape clears it - see the field's doc.
        // The bare-Escape branch below is the only place that puts a
        // value back.
        let prev_alt_screen_escape = self.last_alt_screen_escape.take();

        // While a tab rename is in progress, intercept all keyboard
        // input: Enter confirms, Escape cancels, other keys edit the name.
        if self.tab_rename_input.is_some() {
            match key.code {
                KeyCode::Enter => {
                    let name = self.tab_rename_input.take().unwrap();
                    if name.is_empty() {
                        self.tab_names.remove(&self.active_tab);
                    } else {
                        self.tab_names.insert(self.active_tab, name);
                    }
                }
                KeyCode::Escape => {
                    self.tab_rename_input = None;
                }
                KeyCode::Backspace => {
                    if let Some(input) = &mut self.tab_rename_input {
                        input.pop();
                    }
                }
                KeyCode::Char(c) if !key.ctrl && !key.alt => {
                    if let Some(input) = &mut self.tab_rename_input {
                        input.push(c);
                    }
                }
                _ => {}
            }
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // While a new-theme name is being entered, intercept all keyboard
        // input the same way: Enter confirms, Escape cancels, other keys
        // edit the name.
        if self.theme_name_input.is_some() {
            match key.code {
                KeyCode::Enter => {
                    let name = self.theme_name_input.take().unwrap();
                    self.create_named_theme(&name);
                }
                KeyCode::Escape => {
                    self.theme_name_input = None;
                }
                KeyCode::Backspace => {
                    if let Some(input) = &mut self.theme_name_input {
                        input.pop();
                    }
                }
                KeyCode::Char(c) if !key.ctrl && !key.alt => {
                    if let Some(input) = &mut self.theme_name_input {
                        input.push(c);
                    }
                }
                _ => {}
            }
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // While the settings page is up it owns all input: edits apply
        // live, Enter/Escape close it, and every key is swallowed so none
        // reaches the PTY.
        if self.settings_page.is_some() {
            let logical = event.logical_key.clone();
            let mut page = self.settings_page.take().unwrap();
            if self.handle_settings_input(&mut page, &logical) {
                self.settings_page = Some(page);
            } else {
                self.on_settings_closed();
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Esc dismisses an open menu before anything else acts on it.
        if self.open_menu.is_some() && key.code == KeyCode::Escape {
            self.close_menu();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Esc clears a mouse-drag selection before anything else acts on it.
        if escape_clears_selection(&key, mode, self.selection.is_some()) {
            self.selection = None;
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Global app shortcuts (settings, font size, tab/pane management,
        // palette toggles): configurable via the `window` block in
        // keybindings.kdl, and checked here so they intercept even while
        // an overlay (palette, tab rename, settings) would otherwise own
        // the key. Window-layout chords (split/close/focus/scroll/zoom)
        // are resolved later, once no overlay claims the key.
        if let Some(action) = self.window_keymap.global_action(&key) {
            self.handle_action(action, focused);
            if self.exit_requested {
                self.exit_requested = false;
                self.quit(event_loop);
                return;
            }
            self.update_window_title();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        if self.palette.is_some() {
            let key = event.logical_key.clone();
            let mut palette = self.palette.take().unwrap();
            self.handle_palette_input(&mut palette, &key, &event.physical_key, focused);
            if palette.active {
                self.palette = Some(palette);
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Escape in Insert mode: forwarded to the PTY if a foreground process
        // is running (a full-screen app, via `is_at_prompt`, or on Linux any
        // other foreground process group leader) or the pane is mid the
        // shell's own tab-completion; otherwise, at a bare shell prompt with
        // no completion in progress, it switches straight to Normal mode.
        // A second bare Escape on the same pane, arriving within
        // `ALT_SCREEN_ESCAPE_DOUBLE_TAP` of one that was forwarded, switches
        // to Normal mode instead of forwarding again - see
        // `last_alt_screen_escape`.
        let bare_esc = mode == Mode::Insert
            && key.code == KeyCode::Escape
            && !key.alt
            && !key.ctrl
            && !key.shift;
        if bare_esc {
            let has_foreground_process = self
                .panes
                .get(&focused)
                .is_some_and(|p| p.has_foreground_process());
            // Cleared unconditionally: a completion this Escape didn't
            // forward to (has_foreground_process was already true) is
            // stale by the time another bare Escape arrives, and one it
            // did forward to is now the shell's problem to resolve, not
            // this app's - either way the next Escape starts fresh.
            let pending_tab_completion = self.pending_tab_completion.remove(&focused);
            let now = Instant::now();
            let double_tap = is_alt_screen_escape_double_tap(prev_alt_screen_escape, focused, now);
            if !double_tap
                && escape_forwarded_to_pty(has_foreground_process, pending_tab_completion)
            {
                self.last_alt_screen_escape = Some((focused, now));
                if let Some(pane) = self.panes.get_mut(&focused) {
                    pane.write(&[0x1b]);
                }
            } else {
                let switch = input::Action::SwitchMode(
                    Mode::Insert.apply(crate::model::mode::ModeEvent::EnterNormal),
                );
                self.handle_action(switch, focused);
            }
            self.update_window_title();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        let at_prompt = self.panes.get(&focused).is_some_and(|p| p.is_at_prompt());
        let kitty_flags = self
            .panes
            .get(&focused)
            .map(|p| p.kitty_flags())
            .unwrap_or(0);
        let modify_other_keys = self
            .panes
            .get(&focused)
            .map(|p| p.modify_other_keys())
            .unwrap_or(None);
        let is_alt_screen = self
            .panes
            .get(&focused)
            .map(|p| p.grid().is_alt_screen())
            .unwrap_or(false);
        let prev_pending = self.pending;
        let action = input::resolve_with(
            mode,
            &key,
            &mut self.pending,
            &self.window_keymap,
            kitty_flags,
            modify_other_keys,
            is_alt_screen,
        );
        if self.pending != prev_pending {
            self.pending_since = if self.pending.hint().is_some() {
                Some(Instant::now())
            } else {
                None
            };
        } else if self.pending == input::PendingPrefix::None {
            self.pending_since = None;
        }
        // Keep the per-pane prompt shadow in step with this key so
        // `Ctrl-/`/`Ctrl-\` can replay edits. Only forwarded keys mutate
        // the line (`apply_insert_key` models them or desyncs on the
        // unmodeled ones); `Edit`/undo/redo update it in their handlers.
        // Mode switches, focus and tab commands leave this line untouched,
        // so the shadow is preserved (undo still works in Normal mode).
        if mode == Mode::Insert && forwarded_to_pty(&action) {
            self.record_insert_key(focused, &key, &action, at_prompt);
            // Deliberately does NOT queue an automatic switch to
            // Normal mode once a submitted command's new prompt
            // settles - Insert mode always resumes after running a
            // command, exactly as it was before that command ran;
            // only an explicit mode-switch chord enters Normal mode.
        }
        self.handle_action(action, focused);
        if self.exit_requested {
            self.exit_requested = false;
            self.quit(event_loop);
            return;
        }
        self.update_window_title();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Route a mouse button press or release. The tabbar and any open menu
    /// take precedence over the panes, so a click on chrome never reaches a
    /// pane underneath it.
    fn on_mouse_input(
        &mut self,
        state: ElementState,
        button: MouseButton,
        event_loop: &ActiveEventLoop,
    ) {
        let focused = self.tab().focused();

        // Tabbar/menubar clicks take precedence over the panes (including
        // mouse-tracking apps), and any click resolves an open menu.
        if let (ElementState::Pressed, MouseButton::Left) = (state, button) {
            let (x, y) = self.cursor_pos;
            if let Some(direction) = self.edge_resize_direction(x, y) {
                if let Some(window) = &self.window {
                    let _ = window.drag_resize_window(direction);
                }
                return;
            }
            if (self.open_menu.is_some() || y < self.top_chrome_height())
                && self.handle_tabbar_click(x, y)
            {
                if self.exit_requested {
                    self.exit_requested = false;
                    self.quit(event_loop);
                    return;
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return;
            }
        }

        // Ctrl+click on a hyperlinked cell opens the URL in the browser.
        // Double-check the scheme even though `osc_dispatch` already
        // filtered it, as defense in depth.
        if state == ElementState::Pressed
            && button == MouseButton::Left
            && self.modifiers.state().control_key()
        {
            if let Some(url) = &self.hovered_url {
                let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
                if matches!(scheme.as_str(), "http" | "https" | "mailto") {
                    let _ = open::that(url);
                    return;
                }
            }
        }

        // Plain clicks always drive Winter's own selection, even over a
        // full-screen app (vim, htop) that has turned on mouse reporting.
        // Hold Shift to forward the click to that app's mouse handling instead.
        let mouse_active = self.panes.get(&focused).is_some_and(|p| p.mouse_tracking())
            && self.modifiers.state().shift_key();

        if mouse_active {
            self.forward_mouse_event(state, button, focused);
            return;
        }

        // Right-click: paste if configured, otherwise open the context menu.
        if state == ElementState::Pressed && button == MouseButton::Right {
            if self.config.paste_on_right_click {
                self.paste_from_clipboard();
            } else {
                let (x, y) = self.cursor_pos;
                self.open_context_menu(x, y);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            return;
        }

        // Any left-press dismisses an open context menu before other handling.
        if state == ElementState::Pressed
            && button == MouseButton::Left
            && self.context_menu_pos.is_some()
        {
            let (x, y) = self.cursor_pos;
            let surface_w = self.viewport_rect().width;
            if let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) {
                let tabbar = self.build_top_tabbar();
                let hit = winter_render::hit_test(&tabbar, surface_w, cw, ch, x, y);
                if let winter_render::TabbarHit::ContextMenuItem(i) = hit {
                    if let Some(action) = self.context_menu_actions.get(i).cloned() {
                        self.close_context_menu();
                        match action {
                            ContextAction::Copy => self.copy_selection(),
                            ContextAction::Paste => self.paste_from_clipboard(),
                            ContextAction::OpenLink(url) => {
                                let scheme =
                                    url.split(':').next().unwrap_or("").to_ascii_lowercase();
                                if matches!(scheme.as_str(), "http" | "https" | "mailto") {
                                    let _ = open::that(&url);
                                }
                            }
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                        return;
                    }
                }
            }
            self.close_context_menu();
        }

        match (state, button) {
            (ElementState::Pressed, MouseButton::Left) => {
                self.mouse_down = true;
                self.selection = None;

                let (x, y) = self.cursor_pos;

                // Divider click: start a drag; skip scrollbar/focus changes.
                let on_divider = {
                    let viewport = self.content_viewport();
                    self.tab().divider_at(x, y, viewport).is_some()
                };
                if on_divider {
                    self.divider_drag = Some((x, y));
                } else {
                    // Scrollbar click: right-edge strip of a pane navigates scrollback.
                    let vp = self.viewport_rect();
                    let layout_vp =
                        crate::model::layout::Rect::new(vp.x, vp.y, vp.width, vp.height);
                    'scroll: for (id, rect) in self.tab().rects(layout_vp) {
                        let pr = Self::layout_rect_to_pane(rect);
                        let sb_x = pr.x + pr.width - SCROLLBAR_CLICK_WIDTH;
                        if x >= sb_x && x <= pr.x + pr.width && y >= pr.y && y < pr.y + pr.height {
                            if let Some(pane) = self.panes.get_mut(&id) {
                                let sbl = pane.grid().scrollback_len();
                                if sbl > 0 {
                                    let rows = pane.grid().rows();
                                    let total = (rows + sbl) as f32;
                                    let frac = ((y - pr.y) / pr.height).clamp(0.0, 1.0);
                                    let top_virtual = (frac * total) as usize;
                                    let new_offset = sbl.saturating_sub(top_virtual);
                                    pane.grid_mut().set_scroll_offset(new_offset);
                                    self.scrollbar_drag = Some(id);
                                    if let Some(window) = &self.window {
                                        window.request_redraw();
                                    }
                                    break 'scroll;
                                }
                            }
                        }
                    }

                    if let Some((pane_id, pane_rect)) = self.pane_at_pixel(x, y) {
                        self.tab_mut().focus(pane_id);
                        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
                        // A click parks the traversal cursor under the
                        // pointer, so selecting with the mouse moves the
                        // cursor instead of leaving it wherever the last
                        // keyboard motion stopped.
                        self.track_nav_cursor_to_mouse(pane_id, row, col);
                        let now = Instant::now();
                        if let Some((prev_time, prev_x, prev_y)) = self.last_click {
                            let dist = ((x - prev_x).powi(2) + (y - prev_y).powi(2)).sqrt();
                            if now.duration_since(prev_time) < Duration::from_millis(400)
                                && dist < 5.0
                            {
                                self.select_word_at(pane_id, row, col);
                            }
                        }
                        self.last_click = Some((now, x, y));
                    }
                }

                self.dirty = true;
            }
            (ElementState::Released, MouseButton::Left) => {
                self.mouse_down = false;
                self.divider_drag = None;
                self.scrollbar_drag = None;
                self.finalize_tab_drag();
                self.copy_selection();
                self.copy_selection_to_primary();
            }
            (ElementState::Pressed, MouseButton::Middle) => {
                self.paste_from_primary();
            }
            _ => {}
        }
    }

    /// Track the pointer: hover highlights, drag-selection extension, split
    /// dragging, and the resize-edge cursor shape.
    fn on_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        let x = position.x as f32;
        let y = position.y as f32;
        self.cursor_pos = (x, y);

        // Divider drag: highest priority, blocks selection and PTY forwarding.
        if self.mouse_down {
            if let Some((prev_x, prev_y)) = self.divider_drag {
                let dx = x - prev_x;
                let dy = y - prev_y;
                let viewport = self.content_viewport();
                if self
                    .tab_mut()
                    .drag_divider(prev_x, prev_y, dx, dy, viewport)
                {
                    self.divider_drag = Some((x, y));
                    self.resize_all_panes();
                    self.dirty = true;
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                return;
            }

            // Scrollbar drag: update scroll position proportionally.
            if let Some(sb_pane_id) = self.scrollbar_drag {
                let vp = self.viewport_rect();
                let layout_vp = crate::model::layout::Rect::new(vp.x, vp.y, vp.width, vp.height);
                if let Some((_, rect)) = self
                    .tab()
                    .rects(layout_vp)
                    .into_iter()
                    .find(|(id, _)| *id == sb_pane_id)
                {
                    let pr = Self::layout_rect_to_pane(rect);
                    if let Some(pane) = self.panes.get_mut(&sb_pane_id) {
                        let sbl = pane.grid().scrollback_len();
                        if sbl > 0 {
                            let rows = pane.grid().rows();
                            let total = (rows + sbl) as f32;
                            let frac = ((y - pr.y) / pr.height).clamp(0.0, 1.0);
                            let top_virtual = (frac * total) as usize;
                            pane.grid_mut()
                                .set_scroll_offset(sbl.saturating_sub(top_virtual));
                            self.dirty = true;
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        }
                    }
                }
                return;
            }
        }

        if self.open_menu.is_some() {
            self.update_menu_hover(x, y);
        }
        if self.context_menu_pos.is_some() {
            self.update_context_menu_hover(x, y);
        }

        // Update tabbar hover state so the renderer can show hover highlights.
        if let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) {
            let surface_w = self.viewport_rect().width;
            let tabbar = self.build_top_tabbar();
            let hit = winter_render::hit_test(&tabbar, surface_w, cw, ch, x, y);
            if hit != self.tabbar_hover {
                self.tabbar_hover = hit;
                if let winter_render::TabbarHit::Tab(_) = hit {
                    self.tab_hover_pos = Some(self.cursor_pos);
                } else {
                    self.tab_hover_pos = None;
                }
                self.dirty = true;
                self.update_window_title();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }

        let focused = self.tab().focused();
        if self.mouse_down
            && self
                .panes
                .get(&focused)
                .is_some_and(|p| p.mouse_drag_tracking())
            && self.modifiers.state().shift_key()
        {
            self.forward_mouse_motion(focused);
            return;
        }

        if self.mouse_down {
            if let Some((pane_id, pane_rect)) = self.pane_at_pixel(x, y) {
                let (row, col) = self.pixel_to_cell(x, y, pane_rect);
                // Selection rows are absolute (see `Grid::to_absolute_row`)
                // so they keep naming the same line if auto-scroll (or a
                // wheel scroll) moves the view mid-drag.
                let abs_row = self
                    .panes
                    .get(&pane_id)
                    .map(|p| p.grid().to_absolute_row(row))
                    .unwrap_or(row);
                if let Some(sel) = &mut self.selection {
                    sel.end_row = abs_row;
                    sel.end_col = col;
                    sel.pane = pane_id;
                } else {
                    self.selection = Some(Selection {
                        block: false,
                        start_row: abs_row,
                        start_col: col,
                        end_row: abs_row,
                        end_col: col,
                        pane: pane_id,
                    });
                }
                // The traversal cursor rides along with the drag's
                // live end, so the block cursor keeps following the
                // mouse while the selection extends.
                self.track_nav_cursor_to_mouse(pane_id, row, col);
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }

        // Update hovered hyperlink and cursor icon (divider resize or pointer).
        let new_url = self.hovered_link_at(x, y);
        self.hovered_url = new_url;
        let vp = self.content_viewport();
        let icon = if let Some(direction) = self.edge_resize_direction(x, y) {
            CursorIcon::from(direction)
        } else {
            match self.tab().divider_at(x, y, vp) {
                Some(crate::model::layout::Direction::Vertical) => CursorIcon::EwResize,
                Some(crate::model::layout::Direction::Horizontal) => CursorIcon::NsResize,
                None => {
                    if self.hovered_url.is_some() {
                        CursorIcon::Pointer
                    } else {
                        CursorIcon::Default
                    }
                }
            }
        };
        if let Some(window) = &self.window {
            window.set_cursor(icon);
        }
    }

    /// Scroll the focused pane, or forward the wheel to the PTY when a
    /// full-screen application has asked for mouse reporting.
    fn on_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let scroll_lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => {
                (y * SCROLL_LINES_PER_WHEEL_NOTCH).round() as isize
            }
            MouseScrollDelta::PixelDelta(pos) => (-pos.y / APPROX_CELL_HEIGHT as f64) as isize,
        };

        let focused = self.tab().focused();
        if self.panes.get(&focused).is_some_and(|p| p.mouse_tracking()) {
            self.forward_mouse_scroll(scroll_lines, focused);
            return;
        }

        if scroll_lines != 0 {
            if let Some(pane) = self.panes.get_mut(&focused) {
                // Alt-screen apps (vim, less, etc.) own their viewport — send
                // arrow keys so they respond to the scroll gesture instead of
                // us scrolling their non-existent scrollback.
                if pane.grid().is_alt_screen() {
                    let arrow = if scroll_lines > 0 {
                        b"\x1b[A" as &[u8]
                    } else {
                        b"\x1b[B"
                    };
                    let count = scroll_lines.unsigned_abs();
                    for _ in 0..count {
                        pane.write(arrow);
                    }
                } else {
                    let grid = pane.grid_mut();
                    if scroll_lines > 0 {
                        grid.scroll_up_history(scroll_lines as usize);
                    } else {
                        grid.scroll_down_history((-scroll_lines) as usize);
                    }
                }
            }
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }
}

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
        self.last_activity = Instant::now();
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
                        self.snap_window_to_cell_grid();
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
                            self.save_app_state();
                        }
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
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
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
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
                    self.pending_since = None;
                }
                if self.window_focused != focused {
                    self.window_focused = focused;
                    self.dirty = true;
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                let focused_pane = self.tab().focused();
                if let Some(pane) = self.panes.get(&focused_pane) {
                    if pane.focus_event() {
                        let seq = if focused { "\x1b[I" } else { "\x1b[O" };
                        if let Some(pane) = self.panes.get_mut(&focused_pane) {
                            pane.write(seq.as_bytes());
                        }
                    }
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
                        self.dirty = true;
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
                    }
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(target_os = "linux")]
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }

        if self.reload_config_if_changed() {
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        if let Some(rx) = &self.control_rx {
            if let Ok(ControlMessage::Reload) = rx.try_recv() {
                self.pending_reload = true;
            }
        }
        self.process_webview_height_reports();
        self.webview_mgr.flush_due_tile_updates();
        if self.pending_reload {
            self.pending_reload = false;
            self.reload(event_loop);
            return;
        }

        self.reap_dead_panes();
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
                let focused = self.tab().focused();
                if self.modes.get(&focused) == Some(&Mode::Normal) {
                    self.init_nav_cursor(focused);
                }
                self.nav_resync_pending = false;
            }
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            self.update_window_title();
        }

        // Once a notice expires, redraw once to clear it from the bar.
        if self.notice.is_some() && self.active_notice().is_none() {
            self.notice = None;
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        // Flip cursor blink phase when the timer fires, requesting a redraw to
        // show the updated state.
        if self.config.cursor.blink {
            let now = Instant::now();
            if now >= self.blink_next_flip {
                self.blink_phase = !self.blink_phase;
                self.blink_next_flip = now + CURSOR_BLINK_PERIOD;
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }

        // Trigger a redraw when the which-key hint timer (~1s) expires.
        if let Some(since) = self.pending_since {
            if self.pending.hint().is_some()
                && since.elapsed() >= std::time::Duration::from_millis(1000)
            {
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }

        self.auto_scroll_selection();

        let now = Instant::now();
        let poll_interval = if is_poll_idle(self.last_activity, now) {
            PTY_POLL_INTERVAL_IDLE
        } else {
            PTY_POLL_INTERVAL
        };
        let next_poll = now + poll_interval;
        let mut wakeup = if self.config.cursor.blink {
            next_poll.min(self.blink_next_flip)
        } else {
            next_poll
        };
        if let Some(since) = self.pending_since {
            if self.pending.hint().is_some() {
                let hint_deadline = since + std::time::Duration::from_millis(1000);
                if hint_deadline > now {
                    wakeup = wakeup.min(hint_deadline);
                }
            }
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
    fn test_render_window_title_expands_pane_placeholders() {
        let mut app = App::new();
        let focused = app.tabs[0].focused();
        // An OSC 0/2 title set by the running app (e.g. butterfly naming its
        // open file) resolves into both `pane_title` and the tab `title`.
        app.pane_titles
            .insert(focused, "butterfly — Winter Term.md".into());

        app.config.window_title_template = "{{ app_name }} — {{ pane_title }} [{{ title }}]".into();
        // No PTY is attached in this fixture, so app_name stays empty.
        assert_eq!(
            app.render_window_title(),
            " — butterfly — Winter Term.md [butterfly — Winter Term.md]"
        );

        app.config.window_title_template = "Winter - {{ title }}".into();
        assert_eq!(
            app.render_window_title(),
            "Winter - butterfly — Winter Term.md"
        );
    }

    #[test]
    fn test_resize_all_panes_keeps_a_scrolled_back_pane_in_place() {
        // Regression: `Grid::resize` returns the view to the live bottom, so the
        // one-row resize that comes with showing/hiding the search status bar used
        // to yank a scrolled-back pane down to the prompt mid-browse.
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let focused = app.tab().focused();
        let mut pane = Pane::with_command(
            40,
            8,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        for _ in 0..20 {
            pane.grid_mut().line_feed();
        }
        pane.grid_mut().scroll_up_history(6);
        app.panes.insert(focused, pane);

        app.resize_all_panes();

        assert_ne!(
            app.panes[&focused].grid().rows(),
            8,
            "fixture must actually be resized for this to prove anything"
        );
        assert_eq!(
            app.panes[&focused].grid().scroll_offset(),
            6,
            "the pane should stay where it was scrolled to"
        );
    }

    #[test]
    fn test_prompt_editing_is_enabled_by_default() {
        let app = App::new();
        assert!(app.prompt_editing_enabled());
    }

    #[test]
    fn test_prompt_editing_is_declined_under_non_emacs_bindings() {
        // The operators are realized as readline chords, so a shell in vi mode
        // has them bound to something else. Declining leaves the user with the
        // shell's own Vim editing; translating anyway would fire whatever
        // `Ctrl-W` happens to mean there.
        let mut app = App::new();
        app.config.prompt_edit_bindings = crate::config::PromptEditBindings::None;
        assert!(!app.prompt_editing_enabled());
    }

    #[test]
    fn test_declined_prompt_operator_writes_nothing_to_the_pty() {
        // The important half: a declined operator must not send stray control
        // bytes to a shell that would misinterpret them.
        let mut app = App::new();
        app.config.prompt_edit_bindings = crate::config::PromptEditBindings::None;
        let focused = app.tab().focused();
        app.delete_on_prompt(crate::app::prompt_edit::PromptDelete::WordBack, focused);
        assert!(
            app.notice.is_some(),
            "the declined keypress should be reported, not silently dropped"
        );
    }

    #[test]
    fn test_entering_insert_scrolls_back_down_to_the_prompt() {
        // Typing lands at the prompt, so whichever way Insert is entered — `i`,
        // `a`, `o`, Visual's `i`, or a plain mode switch from a menu/click — the
        // pane snaps out of scrolled-back history to the live bottom.
        let entries: Vec<input::Action> = vec![
            input::Action::EnterInsert(input::InsertAt::Cursor),
            input::Action::EnterInsert(input::InsertAt::After),
            input::Action::EnterInsert(input::InsertAt::LineEnd),
            input::Action::SwitchMode(Mode::Insert),
        ];
        for action in entries {
            let mut app = App::new();
            let focused = app.tab().focused();
            let mut pane = Pane::with_command(
                40,
                8,
                portable_pty::CommandBuilder::new("cat"),
                winter_render::MAX_SCROLLBACK,
            )
            .expect("test pane spawn");
            // Build a page of scrollback, then scroll up into it.
            for _ in 0..16 {
                pane.grid_mut().line_feed();
            }
            pane.grid_mut().scroll_up_history(5);
            assert_eq!(
                pane.grid().scroll_offset(),
                5,
                "fixture must start scrolled up"
            );
            app.panes.insert(focused, pane);
            app.modes.insert(focused, Mode::Normal);
            app.set_nav_cursor(focused, (0, 0));

            app.handle_action(action.clone(), focused);

            assert_eq!(
                app.panes[&focused].grid().scroll_offset(),
                0,
                "{action:?} should scroll back down to the prompt"
            );
            assert_eq!(app.modes.get(&focused), Some(&Mode::Insert));
        }
    }

    #[test]
    fn test_escape_in_normal_mode_stays_in_normal_and_only_clears_the_search() {
        // `Esc` used to drop straight back to Insert, handing the next keystroke
        // to the shell mid-navigation. It now behaves like vim's `:nohlsearch`:
        // the search highlight goes, the mode does not.
        let mut app = App::new();
        let focused = app.tab().focused();
        app.modes.insert(focused, Mode::Normal);
        app.search_query = Some("foo".to_string());
        app.search_match_total = 2;

        app.handle_action(input::Action::SearchCancel, focused);

        assert_eq!(app.modes.get(&focused), Some(&Mode::Normal));
        assert!(app.search_query.is_none());
        assert_eq!(app.search_match_total, 0);
    }

    #[test]
    fn test_enter_insert_is_the_only_way_out_of_normal_mode() {
        // `i`, `a` and `o` all reach Insert (they differ only in where the cursor
        // lands, which `move_nav_cursor` handles), from Normal and from Visual.
        for (mode, at) in [
            (Mode::Normal, input::InsertAt::Cursor),
            (Mode::Normal, input::InsertAt::After),
            (Mode::Normal, input::InsertAt::LineEnd),
            (Mode::Visual, input::InsertAt::Cursor),
        ] {
            let mut app = App::new();
            let focused = app.tab().focused();
            app.modes.insert(focused, mode);
            app.set_nav_cursor(focused, (0, 0));

            app.handle_action(input::Action::EnterInsert(at), focused);

            assert_eq!(
                app.modes.get(&focused),
                Some(&Mode::Insert),
                "{at:?} from {mode:?} should reach Insert"
            );
            assert_eq!(app.nav_cursor(focused), None, "the nav cursor is dropped");
        }
    }

    #[test]
    fn test_app_default_has_one_pane() {
        let app = App::new();
        assert_eq!(app.tab().panes(), vec![PaneId(0)]);
        assert!(app.panes.is_empty());
    }

    #[test]
    fn test_nav_cursor_is_isolated_per_pane() {
        // Regression: the Normal-mode traversal cursor used to be a single shared
        // field, so switching focus between two panes leaked the source pane's
        // cursor coordinates into the destination — landing the cursor before the
        // destination pane's first typeable column. Now it is stored per pane, so
        // each pane's cursor is read only for that pane.
        let mut app = App::new();
        let a = PaneId(1);
        let b = PaneId(2);
        app.set_nav_cursor(a, (5, 3));
        app.set_nav_cursor(b, (1, 9));
        // Pane A's cursor never bleeds into pane B and vice versa.
        assert_eq!(app.nav_cursor(a), Some((5, 3)));
        assert_eq!(app.nav_cursor(b), Some((1, 9)));
        // A pane with no cursor reads as None, regardless of other panes' cursors.
        assert_eq!(app.nav_cursor(PaneId(99)), None);
        // Clearing one pane's cursor does not disturb the other's.
        app.clear_nav_cursor(a);
        assert_eq!(app.nav_cursor(a), None);
        assert_eq!(app.nav_cursor(b), Some((1, 9)));
    }

    #[test]
    fn test_alloc_pane_id_is_monotonic() {
        let mut app = App::new();
        assert_eq!(app.alloc_pane_id(), PaneId(1));
        assert_eq!(app.alloc_pane_id(), PaneId(2));
    }

    #[test]
    fn test_content_band_fits_whole_rows_and_never_reaches_into_the_status_bar() {
        let ch = 20.0;
        // Exact fit: no padding.
        assert_eq!(content_band(80.0, ch), (4, 0.0));
        // Sub-row slack is centered above the rows, not handed to the bar.
        assert_eq!(content_band(85.0, ch), (4, 2.0));
        // The Modern tabbar's extra pixels can leave less than a whole row's worth
        // of slack; the band still keeps at least one row and no negative padding,
        // so panes never overlap the bar.
        assert_eq!(content_band(19.0, ch), (1, 0.0));
        assert_eq!(content_band(0.0, ch), (1, 0.0));
        assert_eq!(content_band(100.0, 0.0), (1, 0.0));

        // Whatever the height, the rows plus padding fit inside the band.
        for h in [601.0_f32, 613.0, 640.0] {
            let (rows, pad) = content_band(h, ch);
            assert!(pad + rows as f32 * ch <= h, "band overflows at h={h}");
        }
    }

    #[test]
    fn test_content_rows_reserves_status_bar_and_floors_at_one() {
        assert_eq!(content_rows(24), 24 - STATUS_BAR_ROWS);
        assert_eq!(content_rows(1), 1);
        assert_eq!(content_rows(0), 1);
    }

    #[test]
    fn test_edge_resize_direction_at_prefers_corners_over_edges() {
        let (w, h, border) = (100.0, 80.0, 6.0);
        // Each corner's border band overlaps both an X and a Y edge band; the
        // diagonal direction must win over collapsing to just North/South or
        // just West/East.
        assert_eq!(
            edge_resize_direction_at(0.0, 0.0, w, h, border),
            Some(ResizeDirection::NorthWest)
        );
        assert_eq!(
            edge_resize_direction_at(w, 0.0, w, h, border),
            Some(ResizeDirection::NorthEast)
        );
        assert_eq!(
            edge_resize_direction_at(0.0, h, w, h, border),
            Some(ResizeDirection::SouthWest)
        );
        assert_eq!(
            edge_resize_direction_at(w, h, w, h, border),
            Some(ResizeDirection::SouthEast)
        );
        // Mid-edge points (away from any corner band) resolve to a single
        // cardinal direction.
        assert_eq!(
            edge_resize_direction_at(w / 2.0, 0.0, w, h, border),
            Some(ResizeDirection::North)
        );
        assert_eq!(
            edge_resize_direction_at(w / 2.0, h, w, h, border),
            Some(ResizeDirection::South)
        );
        assert_eq!(
            edge_resize_direction_at(0.0, h / 2.0, w, h, border),
            Some(ResizeDirection::West)
        );
        assert_eq!(
            edge_resize_direction_at(w, h / 2.0, w, h, border),
            Some(ResizeDirection::East)
        );
        // The window's interior, beyond the border on every side, resizes nothing.
        assert_eq!(
            edge_resize_direction_at(w / 2.0, h / 2.0, w, h, border),
            None
        );
    }

    #[test]
    fn test_snap_height_to_rows_lands_on_a_whole_number_of_rows() {
        let (top_h, status_h, ch) = (40.0, 20.0, 19.0);
        // 682.8px content area (h - top_h - status_h = 622.8) floors to 32
        // rows with a leftover fraction; snapping must round to the nearest
        // row boundary, not floor (which would shrink the window on every
        // resize instead of settling on the closer fit).
        let snapped = snap_height_to_rows(682.8, top_h, status_h, ch);
        assert_eq!(snapped, top_h + 33.0 * ch + status_h);

        // Exact fits are left untouched.
        let exact = top_h + 10.0 * ch + status_h;
        assert_eq!(snap_height_to_rows(exact, top_h, status_h, ch), exact);

        // Never rounds down to zero rows, even for a height smaller than the
        // chrome itself.
        assert_eq!(
            snap_height_to_rows(10.0, top_h, status_h, ch),
            top_h + ch + status_h
        );
    }

    #[test]
    fn test_snap_height_to_rows_survives_a_fractional_pixel_top_chrome() {
        // Regression: the Modern tabbar's height is a fractional number of
        // pixels (e.g. 36.2), so the exact height for a row count is also
        // fractional (36.2 + 33 * 19.0 = 663.2). Rounding to *nearest* landed
        // on 663, and `content_band`'s `floor()` at render time then read
        // that as only 32 rows, dumping the whole missing 0.2px's neighboring
        // row back in as ~9px of padding on each side. Snapping must round
        // up, so the snapped height always contains the intended row count.
        let (top_h, status_h, ch) = (36.2_f32, 0.0, 19.0);
        let snapped = snap_height_to_rows(665.0, top_h, status_h, ch);
        let (rows, pad) = content_band(snapped - top_h - status_h, ch);
        assert_eq!(
            rows, 33,
            "the snapped height must floor back to the row count it targeted"
        );
        assert_eq!(
            pad, 0.0,
            "a correctly snapped height leaves no padding to center"
        );
    }

    #[test]
    fn test_snap_width_to_cols_survives_a_fractional_cell_width() {
        // Same failure mode as the row case, but on the column axis: real
        // cell widths are rarely whole pixels (font-metric derived), so
        // rounding to *nearest* can land a fraction of a pixel under the
        // exact width and lose the whole column `grid_size_for`'s floor was
        // aiming to keep, showing up as leftover slack past the last column.
        let (h_pad, cw) = (4.0_f32, 9.42);
        let snapped = snap_width_to_cols(522.0, h_pad, cw);
        let cols = ((snapped - h_pad) / cw).floor() as usize;
        assert_eq!(
            cols, 55,
            "the snapped width must floor back to the column count it targeted"
        );
    }

    #[test]
    fn test_status_bar_visible_forces_on_during_search_even_when_configured_off() {
        let mut app = App::new();
        app.config.status_bar.enabled = false;
        assert!(!app.status_bar_visible());

        app.search_query = Some("foo".to_string());
        assert!(app.status_bar_visible());

        app.search_query = None;
        assert!(!app.status_bar_visible());
    }

    #[test]
    fn test_status_bar_visible_stays_on_when_configured_on_regardless_of_search() {
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        assert!(app.status_bar_visible());

        app.search_query = Some("foo".to_string());
        assert!(app.status_bar_visible());
    }

    #[test]
    fn test_leaving_normal_mode_ends_a_search_and_hides_the_forced_status_bar() {
        let mut app = app_with_tabs(1);
        app.config.status_bar.enabled = false;
        let focused = app.tab().focused();
        app.modes.insert(focused, Mode::Normal);

        // Search executed, now browsing matches in Normal mode: the bar is
        // forced visible even though it's configured off.
        app.search_query = Some("foo".to_string());
        app.search_match_index = 1;
        app.search_match_total = 3;
        assert!(app.status_bar_visible());

        // `i` back to Insert (done browsing, not a `SearchCancel` mid-input)
        // still ends the search and lets the bar drop back to hidden.
        app.handle_action(input::Action::SwitchMode(Mode::Insert), focused);
        assert!(app.search_query.is_none());
        assert_eq!(app.search_match_index, 0);
        assert_eq!(app.search_match_total, 0);
        assert!(!app.status_bar_visible());
    }

    #[test]
    fn test_status_bar_labels_each_mode() {
        let theme = Theme::dark();
        let cfg = StatusBarConfig::default();
        assert_eq!(
            status_bar(Mode::Insert, &theme, None, None, &cfg).mode,
            "\u{f03eb} Insert"
        );
        assert_eq!(
            status_bar(Mode::Normal, &theme, None, None, &cfg).mode,
            "\u{e795} Normal"
        );
        assert_eq!(
            status_bar(Mode::BlockFocus, &theme, None, None, &cfg).mode,
            "\u{f0485} Block"
        );
        assert_ne!(
            status_bar(Mode::Insert, &theme, None, None, &cfg).accent,
            status_bar(Mode::Normal, &theme, None, None, &cfg).accent
        );
    }

    #[test]
    fn test_status_bar_show_mode_toggle_hides_mode_label() {
        let theme = Theme::dark();
        let cfg = StatusBarConfig {
            show_mode: false,
            ..StatusBarConfig::default()
        };
        let bar = status_bar(Mode::Normal, &theme, None, None, &cfg);
        assert_eq!(bar.mode, "");
    }

    #[test]
    fn test_status_bar_shows_search_while_active_and_hides_when_none() {
        let theme = Theme::dark();
        let cfg = StatusBarConfig::default();
        let active = status_bar(
            Mode::Normal,
            &theme,
            Some(StatusSearch {
                query: "foo".to_string(),
                match_index: 1,
                match_total: 3,
                reverse: false,
            }),
            None,
            &cfg,
        );
        assert!(active.search.is_some());

        let inactive = status_bar(Mode::Normal, &theme, None, None, &cfg);
        assert!(inactive.search.is_none());
    }

    #[test]
    fn test_winit_key_to_code_chars() {
        let placeholder = PhysicalKey::Code(winit::keyboard::KeyCode::KeyA);
        assert_eq!(
            winit_key_to_code(&Key::Character("a".into()), &placeholder),
            KeyCode::Char('a')
        );
        assert_eq!(
            winit_key_to_code(&Key::Named(NamedKey::Enter), &placeholder),
            KeyCode::Enter
        );
        assert_eq!(
            winit_key_to_code(&Key::Named(NamedKey::Space), &placeholder),
            KeyCode::Space
        );
    }

    #[test]
    fn test_winit_key_to_code_falls_back_to_physical_key_when_logical_is_unidentified() {
        // Windows reports no character at all for some Ctrl+Alt digit/letter
        // combos on certain keyboard layouts; the physical key position must
        // still resolve so those chords aren't silently lost.
        let unidentified =
            Key::Unidentified(winit::keyboard::NativeKeyCode::Windows(0x0032).into());
        let physical = PhysicalKey::Code(winit::keyboard::KeyCode::Digit2);
        assert_eq!(
            winit_key_to_code(&unidentified, &physical),
            KeyCode::Char('2')
        );
    }

    #[test]
    fn test_parse_mux_spawn_query_splits_name_from_command() {
        // The GUI hands the whole prompt line to the parser; whitespace runs
        // between the name and the command must collapse to single spaces,
        // and a bare name must mean "default shell" (None), not an
        // empty-string command the server would try to run.
        assert_eq!(
            parse_mux_spawn_query("dev  cargo   watch -x test"),
            Some(("dev".into(), Some("cargo watch -x test".into())))
        );
        assert_eq!(parse_mux_spawn_query("dev"), Some(("dev".into(), None)));
    }

    #[test]
    fn test_parse_mux_spawn_query_rejects_unusable_names() {
        // Enter on an untouched prompt yields the empty query; a
        // placeholder-style label and a name carrying control characters
        // must all be rejected as spawn names instead of reaching the
        // Spawn message.
        assert_eq!(parse_mux_spawn_query(""), None);
        assert_eq!(parse_mux_spawn_query("   "), None);
        assert_eq!(parse_mux_spawn_query("(default shell)"), None);
        assert_eq!(parse_mux_spawn_query("de\u{7}v cargo"), None);
    }

    #[test]
    fn test_parse_mux_attach_query_splits_host_from_session() {
        // A bare host means "default" session (None), matching the CLI's
        // local-attach default so the two entry points behave the same way.
        assert_eq!(
            parse_mux_attach_query("box.example.com work"),
            Some(("box.example.com".into(), Some("work".into())))
        );
        assert_eq!(
            parse_mux_attach_query("box.example.com"),
            Some(("box.example.com".into(), None))
        );
    }

    #[test]
    fn test_parse_mux_attach_query_rejects_unusable_hosts() {
        assert_eq!(parse_mux_attach_query(""), None);
        assert_eq!(parse_mux_attach_query("   "), None);
        assert_eq!(parse_mux_attach_query("(host [session])"), None);
        assert_eq!(parse_mux_attach_query("ho\u{7}st work"), None);
    }

    #[test]
    fn test_forwarded_to_pty_rejects_empty_send_bytes() {
        // A key winit could not identify encodes to zero bytes (see
        // `winit_key_to_code`'s `Char('\0')` fallback); it must not count as
        // forwarded, or the prompt shadow would wrongly desync/mutate for a key
        // that never reached the shell.
        assert!(!forwarded_to_pty(&Action::SendBytes(Vec::new())));
        assert!(forwarded_to_pty(&Action::SendBytes(vec![0x04])));
        assert!(!forwarded_to_pty(&Action::Ignore));
    }

    #[test]
    fn test_escape_forwarded_to_pty_on_foreground_process_or_pending_completion() {
        // Regression: a bare Escape used to switch straight to Normal mode at
        // any bare shell prompt, even one mid the shell's own tab-completion
        // (e.g. zsh's menu-select) - the completion was left open with Normal
        // mode active underneath it, since the real ESC byte never reached
        // the shell to cancel it.
        assert!(escape_forwarded_to_pty(true, false));
        assert!(escape_forwarded_to_pty(false, true));
        assert!(escape_forwarded_to_pty(true, true));
        assert!(!escape_forwarded_to_pty(false, false));
    }

    #[test]
    fn test_alt_screen_escape_double_tap_requires_same_pane_within_window() {
        // Regression: the second Escape must land on the same pane the first
        // one was forwarded from, and within the double-tap window - an
        // Escape on a different pane, or one that arrives too late, must
        // forward to the full-screen app again rather than switching modes.
        let pane = PaneId(1);
        let other_pane = PaneId(2);
        let first = Instant::now();
        let soon = first + Duration::from_millis(1);
        let late = first + ALT_SCREEN_ESCAPE_DOUBLE_TAP;

        assert!(is_alt_screen_escape_double_tap(
            Some((pane, first)),
            pane,
            soon
        ));
        assert!(!is_alt_screen_escape_double_tap(
            Some((other_pane, first)),
            pane,
            soon
        ));
        assert!(!is_alt_screen_escape_double_tap(
            Some((pane, first)),
            pane,
            late
        ));
        assert!(!is_alt_screen_escape_double_tap(None, pane, soon));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_is_pre_focus_key_leak_swallows_press_before_focus() {
        // Regression: Alt+Tab into the window can dispatch the queued
        // Tab KeyboardInput before the Focused(true) event, so `has_focus()`
        // still reads false when the leaked press arrives.
        assert!(is_pre_focus_key_leak(ElementState::Pressed, false));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_is_pre_focus_key_leak_ignores_press_once_focused() {
        assert!(!is_pre_focus_key_leak(ElementState::Pressed, true));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_is_pre_focus_key_leak_ignores_release_events() {
        // A Released event never reaches the PTY on its own (see
        // `encode_release`), so it must not be swallowed even if it arrives
        // while `has_focus()` still reads false.
        assert!(!is_pre_focus_key_leak(ElementState::Released, false));
    }

    #[test]
    fn test_layout_rect_conversion() {
        let rect = Rect::new(10.0, 20.0, 400.0, 300.0);
        let pane_rect = App::layout_rect_to_pane(rect);
        assert_eq!(pane_rect.x, 10.0);
        assert_eq!(pane_rect.y, 20.0);
    }

    #[test]
    fn test_app_new_has_no_quick_select() {
        let app = App::new();
        assert!(app.quick_select.is_none());
    }

    #[test]
    fn test_set_error_surfaces_a_live_notice() {
        let mut app = App::new();
        assert!(app.active_notice().is_none());
        app.set_error("boom");
        assert_eq!(app.active_notice(), Some(("boom", NoticeKind::Error)));
    }

    #[test]
    fn test_set_notice_surfaces_an_info_notice() {
        let mut app = App::new();
        app.set_notice("Copied to clipboard");
        assert_eq!(
            app.active_notice(),
            Some(("Copied to clipboard", NoticeKind::Info))
        );
    }

    #[test]
    fn test_run_command_action_dispatches_the_same_effect_as_the_palette() {
        // A chord bound to a palette-only command reaches the app as
        // `Action::RunCommand`; `handle_action` must forward it to
        // `run_command` so the effect matches selecting it from the palette.
        let mut app = App::new();
        let focused = app.tab().focused();
        app.handle_action(
            input::Action::RunCommand("mux_new_session".to_string()),
            focused,
        );
        assert_eq!(
            app.palette.as_ref().map(|p| p.mode.clone()),
            Some(PaletteMode::MuxNew)
        );
    }

    #[test]
    fn test_expired_notice_is_not_active() {
        let mut app = App::new();
        // An already-elapsed expiry reads as inactive.
        app.notice = Some((
            "stale".to_string(),
            NoticeKind::Error,
            Instant::now() - Duration::from_secs(1),
        ));
        assert!(app.active_notice().is_none());
    }

    #[test]
    fn test_status_bar_notice_is_set() {
        let theme = Theme::dark();
        let cfg = StatusBarConfig::default();
        let notice = StatusNotice {
            kind: NoticeKind::Info,
            text: "oops".to_string(),
        };
        let bar = status_bar(Mode::Normal, &theme, None, Some(notice), &cfg);
        assert_eq!(bar.notice.map(|n| n.text), Some("oops".to_string()));
    }

    /// An app with `n` empty tabs (no panes/PTYs), active tab 0, MRU `[0, 1, ..]`.
    fn app_with_tabs(n: usize) -> App {
        let mut app = App::new();
        for i in 1..n {
            app.tabs.push(Tab::with_root(PaneId(i as u64)));
        }
        app.tab_mru = (0..n).collect();
        app
    }

    #[test]
    fn test_close_pane_keeps_last_pane_of_only_tab() {
        let mut app = app_with_tabs(1);
        let pane = app.tab().panes()[0];
        app.close_pane(pane);
        // The sole pane of the sole tab is preserved, not closed.
        assert_eq!(app.tabs.len(), 1);
        assert!(app.tab().panes().contains(&pane));
    }

    #[test]
    fn test_new_mux_tab_attaches_and_replays() {
        use crate::mux::protocol::{self, ServerMessage};
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
            std::env::temp_dir().join(format!("winter-app-mux-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let socket_path = path.to_string_lossy().to_string();

        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _attach = read_frame(&mut conn); // pane's Attach
            for frame in [
                protocol::encode(&ServerMessage::Attached {
                    session: "work".into(),
                    cols: 80,
                    rows: 24,
                }),
                protocol::encode(&ServerMessage::Output {
                    session: "work".into(),
                    bytes: b"mux tab live\n".to_vec(),
                }),
            ] {
                conn.write_all(&frame).unwrap();
            }
            // Consume the attach-time Resize, then close.
            let _ = read_frame(&mut conn);
        });

        let mut app = App::new();
        app.new_mux_tab_at(&socket_path, "work");
        // The bootstrap tab plus the new mux tab.
        assert_eq!(app.tabs.len(), 2);
        let focused = app.tab().focused();
        assert_eq!(app.panes[&focused].mux_session(), Some("work"));

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut text = String::new();
        while Instant::now() < deadline {
            if let Some(pane) = app.panes.get_mut(&focused) {
                pane.drain_output();
            }
            text = app.panes[&focused].grid().to_text();
            if text.contains("mux tab live") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            text.contains("mux tab live"),
            "attached pane must render session output, got: {text}"
        );
        assert_eq!(app.tab_title(app.active_tab), "mux: work");

        // Detaching closes the pane's tab view; the session itself lives
        // on the server, out of the app's hands.
        app.run_command("mux_detach_session", focused);
        assert_eq!(app.tabs.len(), 1, "the mux tab must be closed");
        assert!(!app.panes.contains_key(&focused));
        assert!(app.active_notice().is_some());

        let _ = server.join();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_close_pane_closes_tab_when_other_tabs_exist() {
        let mut app = app_with_tabs(2);
        app.active_tab = 0;
        let pane = app.tabs[0].panes()[0];
        app.close_pane(pane);
        // The tab held a single pane and another tab exists, so the tab closes.
        assert_eq!(app.tabs.len(), 1);
    }

    /// Inserts a real, already-exited `Pane` at `id` so `reap_dead_panes` sees
    /// it as dead. Waits for the child process to actually terminate rather
    /// than assuming immediate exit.
    fn insert_dead_pane(app: &mut App, id: PaneId) {
        let mut pane = Pane::with_command(20, 5, portable_pty::CommandBuilder::new("true"), 1000)
            .expect("test pane spawn");
        for _ in 0..100 {
            if !pane.is_alive() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        app.panes.insert(id, pane);
    }

    #[test]
    fn test_reap_dead_panes_closes_tab_when_its_lone_pane_dies() {
        let mut app = app_with_tabs(2);
        let dead_id = app.tabs[0].panes()[0];
        insert_dead_pane(&mut app, dead_id);
        app.reap_dead_panes();
        // The dead pane was alone in its tab, so the whole tab closes.
        assert_eq!(app.tabs.len(), 1);
        assert!(!app.tabs.iter().any(|t| t.panes().contains(&dead_id)));
    }

    #[test]
    fn test_reap_dead_panes_requests_exit_when_last_pane_of_last_tab_dies() {
        let mut app = app_with_tabs(1);
        let dead_id = app.tab().panes()[0];
        insert_dead_pane(&mut app, dead_id);
        app.reap_dead_panes();
        // Nothing else to fall back to, so this requests the app quit rather
        // than leaving a pane whose shell has already exited on screen.
        assert!(app.exit_requested);
    }

    #[test]
    fn test_switch_tab_moves_to_front_of_mru() {
        let mut app = app_with_tabs(3);
        app.switch_tab(2);
        assert_eq!(app.active_tab, 2);
        assert_eq!(app.tab_mru, vec![2, 0, 1]);
        app.switch_tab(1);
        assert_eq!(app.tab_mru, vec![1, 2, 0]);
        assert_eq!(app.mru_walk, None);
    }

    #[test]
    fn test_recent_tab_walks_backward_and_forward_without_reshuffling() {
        let mut app = app_with_tabs(3);
        app.switch_tab(2);
        app.switch_tab(1); // MRU now [1, 2, 0], current tab 1.

        // Backward steps toward less-recently-used, holding the order still.
        app.recent_tab(false);
        assert_eq!(app.active_tab, 2);
        assert_eq!(app.mru_walk, Some(1));
        assert_eq!(app.tab_mru, vec![1, 2, 0]);
        app.recent_tab(false);
        assert_eq!(app.active_tab, 0);
        assert_eq!(app.mru_walk, Some(2));

        // Forward steps back toward more-recently-used.
        app.recent_tab(true);
        assert_eq!(app.active_tab, 2);
        assert_eq!(app.tab_mru, vec![1, 2, 0]);
    }

    #[test]
    fn test_deliberate_switch_ends_walk_and_reseeds_mru() {
        let mut app = app_with_tabs(3);
        app.switch_tab(1); // MRU [1, 0, 2], current tab 1.
        app.recent_tab(false); // Walk to tab 0 (a less-recent tab).
        assert_eq!(app.active_tab, 0);
        assert!(app.mru_walk.is_some());
        // A deliberate switch to a different tab ends the walk and re-seeds.
        app.switch_tab(2);
        assert_eq!(app.mru_walk, None);
        assert_eq!(app.tab_mru[0], 2);
    }

    #[test]
    fn test_recent_tab_is_noop_with_one_tab() {
        let mut app = App::new();
        app.recent_tab(false);
        app.recent_tab(true);
        assert_eq!(app.active_tab, 0);
        assert_eq!(app.mru_walk, None);
    }

    #[test]
    fn test_close_tab_compacts_mru_indices() {
        let mut app = app_with_tabs(3);
        app.switch_tab(2); // MRU [2, 0, 1], current tab 2.
        app.close_tab(0); // Tabs above the closed index shift down by one.
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab, 1);
        // The closed index is gone and 1->0, 2->1; current tab re-seeded to front.
        assert_eq!(app.tab_mru, vec![1, 0]);
        assert_eq!(app.mru_walk, None);
    }

    #[test]
    fn test_swap_tabs_updates_active_tab_and_mru() {
        let mut app = app_with_tabs(3);
        app.switch_tab(1); // active = 1, MRU [1, 0, 2]
        let tab0_id = app.tabs[0].focused();
        let tab1_id = app.tabs[1].focused();
        app.swap_tabs(0, 1);
        // Content moved: tab1_id is now at index 0, tab0_id at index 1.
        assert_eq!(app.tabs[0].focused(), tab1_id);
        assert_eq!(app.tabs[1].focused(), tab0_id);
        // active_tab follows the dragged content (was 1, src was 0, so active goes 0→nope:
        // we were on tab 1, swapping 0 and 1 → active was 1 → now 0).
        assert_eq!(app.active_tab, 0);
        // MRU: [1,0,2] → indices swapped → [0,1,2]
        assert_eq!(app.tab_mru[0], 0);
    }

    #[test]
    fn test_swap_tabs_is_noop_for_same_index() {
        let mut app = app_with_tabs(2);
        let tab0_id = app.tabs[0].focused();
        app.swap_tabs(0, 0);
        assert_eq!(app.tabs[0].focused(), tab0_id);
        assert_eq!(app.active_tab, 0);
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
    fn test_switch_to_pane_changes_tab_and_focus() {
        let mut app = App::new();
        assert_eq!(app.active_tab, 0);

        // Add another tab (creates tab index 1)
        app.new_tab();
        assert_eq!(app.tabs.len(), 2);

        // Switch back to tab index 0
        app.switch_tab(0);
        assert_eq!(app.active_tab, 0);

        // Switch to the pane in tab index 1
        let second_tab_pane_id = app.tabs[1].focused();
        app.switch_to_pane(second_tab_pane_id);
        assert_eq!(app.active_tab, 1);
        assert_eq!(app.tabs[1].focused(), second_tab_pane_id);
    }

    #[test]
    fn test_escape_clears_selection_outside_visual_mode() {
        let key = input::Key {
            alt: false,
            code: KeyCode::Escape,
            ctrl: false,
            shift: false,
        };
        assert!(escape_clears_selection(&key, Mode::Insert, true));
        assert!(escape_clears_selection(&key, Mode::Normal, true));
    }

    #[test]
    fn test_escape_clears_selection_false_without_a_selection() {
        let key = input::Key {
            alt: false,
            code: KeyCode::Escape,
            ctrl: false,
            shift: false,
        };
        assert!(!escape_clears_selection(&key, Mode::Insert, false));
    }

    #[test]
    fn test_escape_clears_selection_false_in_visual_mode() {
        // Regression: Visual mode's own Escape handling (the Normal-mode
        // switch in `handle_action`) already clears the selection together
        // with the visual anchor; double-handling it here would be redundant
        // and, if the ordering ever changed, could race that clear.
        let key = input::Key {
            alt: false,
            code: KeyCode::Escape,
            ctrl: false,
            shift: false,
        };
        assert!(!escape_clears_selection(&key, Mode::Visual, true));
    }

    #[test]
    fn test_escape_clears_selection_false_with_modifiers() {
        let shift_escape = input::Key {
            alt: false,
            code: KeyCode::Escape,
            ctrl: false,
            shift: true,
        };
        assert!(!escape_clears_selection(&shift_escape, Mode::Insert, true));
    }

    /// A pane with enough scrollback (a `seq` run wider than the viewport) to
    /// exercise auto-scroll against real history rather than the live-bottom
    /// edge case. Mirrors `navigation::tests::pane_with_scrollback`.
    fn pane_with_scrollback() -> Pane {
        let mut pane = Pane::with_command(
            40,
            10,
            portable_pty::CommandBuilder::new("bash"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        pane.write(b"seq 1 200\n");
        for _ in 0..100 {
            pane.drain_output();
            if pane.grid().scrollback_len() > pane.grid().rows() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            pane.grid().scrollback_len() > pane.grid().rows(),
            "fixture needs more than a page of scrollback"
        );
        pane
    }

    #[test]
    fn test_auto_scroll_selection_scrolls_up_when_pointer_near_top_edge() {
        let mut app = App::new();
        let id = PaneId(1);
        app.panes.insert(id, pane_with_scrollback());
        app.mouse_down = true;
        app.selection = Some(Selection {
            block: false,
            start_row: 5,
            start_col: 0,
            end_row: 5,
            end_col: 0,
            pane: id,
        });
        // Just inside the top edge margin of the (window-less, so default
        // 800x600) viewport — barely deep enough to scroll, so one step moves
        // the minimum one line (see test_auto_scroll_selection_speed_scales_
        // with_edge_depth for the deep-in-margin case).
        app.cursor_pos = (10.0, 23.0);

        app.auto_scroll_selection();

        let pane = &app.panes[&id];
        assert_eq!(
            pane.grid().scroll_offset(),
            1,
            "should scroll one line into history"
        );
        let sel = app.selection.as_ref().expect("selection still active");
        assert_eq!(
            (sel.end_row, sel.end_col),
            (pane.grid().to_absolute_row(0), 0),
            "drag end follows the new top row's absolute address"
        );
    }

    #[test]
    fn test_auto_scroll_selection_expands_further_back_each_tick() {
        // Regression: auto-scroll used to pin the drag's live end to the
        // viewport's fixed row 0, so each tick reinterpreted that same row
        // number as whatever content had just scrolled into it instead of
        // growing the selection to cover everything scrolled past. Since
        // `Selection` rows are absolute, the end should move strictly
        // further from the start on every successful scroll tick.
        let mut app = App::new();
        let id = PaneId(1);
        app.panes.insert(id, pane_with_scrollback());
        app.mouse_down = true;
        let start_abs = app.panes[&id].grid().to_absolute_row(5);
        app.selection = Some(Selection {
            block: false,
            start_row: start_abs,
            start_col: 0,
            end_row: start_abs,
            end_col: 0,
            pane: id,
        });
        app.cursor_pos = (10.0, 23.0);

        let mut ends = Vec::new();
        for _ in 0..3 {
            app.auto_scroll_next = Instant::now(); // Bypass the interval throttle.
            app.auto_scroll_selection();
            ends.push(app.selection.as_ref().unwrap().end_row);
        }

        assert!(
            ends[0] > ends[1] && ends[1] > ends[2],
            "end_row should move further back each tick, not reset: {ends:?}"
        );
        assert_eq!(app.panes[&id].grid().scroll_offset(), 3);
    }

    #[test]
    fn test_auto_scroll_selection_speed_scales_with_edge_depth() {
        // Regression: the drag auto-scroll used to move a fixed one line per
        // tick no matter how deep into the edge margin the pointer sat, which
        // reads as sluggish when reaching for text well above the viewport.
        // The step now scales with depth: barely inside crawls, near the
        // viewport's own edge moves AUTO_SCROLL_MAX_LINES_PER_TICK.
        let shallow = {
            let mut app = App::new();
            let id = PaneId(1);
            app.panes.insert(id, pane_with_scrollback());
            app.mouse_down = true;
            app.selection = Some(Selection {
                block: false,
                start_row: 5,
                start_col: 0,
                end_row: 5,
                end_col: 0,
                pane: id,
            });
            app.cursor_pos = (10.0, 23.0);
            app.auto_scroll_selection();
            app.panes[&id].grid().scroll_offset()
        };
        let deep = {
            let mut app = App::new();
            let id = PaneId(1);
            app.panes.insert(id, pane_with_scrollback());
            app.mouse_down = true;
            app.selection = Some(Selection {
                block: false,
                start_row: 5,
                start_col: 0,
                end_row: 5,
                end_col: 0,
                pane: id,
            });
            app.cursor_pos = (10.0, 0.0);
            app.auto_scroll_selection();
            app.panes[&id].grid().scroll_offset()
        };
        assert_eq!(shallow, 1, "barely inside the margin scrolls one line");
        assert_eq!(
            deep, AUTO_SCROLL_MAX_LINES_PER_TICK,
            "at the viewport's own edge the step hits its cap"
        );
    }

    #[test]
    fn test_selected_text_covers_every_line_after_auto_scrolling_past_one_page() {
        // Regression: `selected_text` used to read rows via `visible_cell`,
        // which only resolves the currently-visible page (`0..rows`). Once a
        // drag's absolute span grew past one page via auto-scroll, rows
        // outside that page read back as blanks instead of the real
        // scrolled-past text. `absolute_cell` fixes this by addressing the
        // scrollback/live buffer directly, independent of scroll position.
        //
        // Content is written directly to the grid (no live shell) so each
        // row holds a known, distinguishable number instead of depending on
        // PTY timing.
        let mut pane = Pane::with_command(10, 3, portable_pty::CommandBuilder::new("true"), 1000)
            .expect("test pane spawn");
        {
            let grid = pane.grid_mut();
            for n in 0..20 {
                for ch in n.to_string().chars() {
                    grid.print(ch);
                }
                grid.carriage_return();
                grid.line_feed();
            }
        }
        let rows = pane.grid().rows();

        let mut app = App::new();
        let id = PaneId(1);
        app.panes.insert(id, pane);
        app.mouse_down = true;
        // Anchor the drag on the live view's current top row (still holding
        // real numbers, not the fresh blank row the cursor just moved to).
        let start_abs = app.panes[&id].grid().to_absolute_row(0);
        app.selection = Some(Selection {
            block: false,
            start_row: start_abs,
            start_col: 0,
            end_row: start_abs,
            end_col: 0,
            pane: id,
        });
        app.cursor_pos = (10.0, 23.0);

        // Scroll well past both a single page's worth of rows and the whole
        // available scrollback, so the offset saturates at the oldest line.
        for _ in 0..(rows + 25) {
            app.auto_scroll_next = Instant::now();
            app.auto_scroll_selection();
        }
        assert_eq!(
            app.panes[&id].grid().scroll_offset(),
            app.panes[&id].grid().scrollback_len(),
            "should have scrolled all the way back"
        );

        let sel = app.selection.as_ref().unwrap();
        let expected_lines = sel.start_row.max(sel.end_row) - sel.start_row.min(sel.end_row) + 1;
        let text = app.selected_text().expect("selection has content");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines.len(),
            expected_lines,
            "should cover every scrolled-through line, not just one page: {lines:?}"
        );
        // The oldest scrolled-into row is "0" (the first line printed), not
        // a blank filler row `visible_cell` would have produced past the
        // live page's bound.
        assert_eq!(lines[0].trim(), "0");
    }

    #[test]
    fn test_auto_scroll_selection_throttles_repeated_ticks() {
        let mut app = App::new();
        let id = PaneId(1);
        app.panes.insert(id, pane_with_scrollback());
        app.mouse_down = true;
        app.selection = Some(Selection {
            block: false,
            start_row: 5,
            start_col: 0,
            end_row: 5,
            end_col: 0,
            pane: id,
        });
        app.cursor_pos = (10.0, 23.0);

        app.auto_scroll_selection();
        app.auto_scroll_selection();

        // The second call lands before `auto_scroll_next`, so it must not
        // advance the scrollback a second time.
        assert_eq!(app.panes[&id].grid().scroll_offset(), 1);
    }

    #[test]
    fn test_auto_scroll_selection_noop_when_pointer_not_near_an_edge() {
        let mut app = App::new();
        let id = PaneId(1);
        app.panes.insert(id, pane_with_scrollback());
        app.mouse_down = true;
        app.selection = Some(Selection {
            block: false,
            start_row: 5,
            start_col: 0,
            end_row: 5,
            end_col: 0,
            pane: id,
        });
        app.cursor_pos = (10.0, 300.0);

        app.auto_scroll_selection();

        assert_eq!(app.panes[&id].grid().scroll_offset(), 0);
    }

    #[test]
    fn test_auto_scroll_selection_noop_when_mouse_not_held() {
        let mut app = App::new();
        let id = PaneId(1);
        app.panes.insert(id, pane_with_scrollback());
        app.mouse_down = false;
        app.selection = Some(Selection {
            block: false,
            start_row: 5,
            start_col: 0,
            end_row: 5,
            end_col: 0,
            pane: id,
        });
        app.cursor_pos = (10.0, 1.0);

        app.auto_scroll_selection();

        assert_eq!(app.panes[&id].grid().scroll_offset(), 0);
    }

    #[test]
    fn test_which_key_timer_logic() {
        let mut app = App::new();
        assert!(app.pending_since.is_none());

        // Opening 'g' prefix starts the timer
        app.pending = input::PendingPrefix::G;
        app.pending_since = Some(Instant::now());

        // Before 1s delay, which-key view is not shown
        let view_before = app
            .pending_since
            .filter(|since| since.elapsed() >= std::time::Duration::from_millis(1000))
            .and_then(|_| {
                app.pending
                    .hint()
                    .map(|(title, items)| winter_render::WhichKeyView {
                        items: items
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                            .collect(),
                        title: title.to_string(),
                    })
            });
        assert!(view_before.is_none(), "hidden before 1s delay");

        // After 1s delay, which-key view is populated
        app.pending_since = Some(Instant::now() - std::time::Duration::from_millis(1005));
        let view_after = app
            .pending_since
            .filter(|since| since.elapsed() >= std::time::Duration::from_millis(1000))
            .and_then(|_| {
                app.pending
                    .hint()
                    .map(|(title, items)| winter_render::WhichKeyView {
                        items: items
                            .iter()
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                            .collect(),
                        title: title.to_string(),
                    })
            });
        assert!(view_after.is_some(), "shown after 1s delay");
        let view = view_after.unwrap();
        assert_eq!(view.title, "g");
        assert!(!view.items.is_empty());

        // Action resolution/cancellation clears prefix and timer
        app.pending = input::PendingPrefix::None;
        app.pending_since = None;
        assert!(app.pending_since.is_none());
    }
}
