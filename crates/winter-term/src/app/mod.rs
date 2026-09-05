//! The native application: winit event loop, GPU renderer, and PTY panes wired
//! together. This is the `Winter` binary's core runtime — keyboard input drives
//! the PTY, PTY output drives the cell grid, and the grid is rendered to the
//! GPU surface every frame.
//!
//! This file holds the `App` struct and the winit event-loop plumbing. Every
//! other responsibility lives in a submodule that carries its own `impl App`:
//!
//! - [`actions`]: keyboard action dispatch.
//! - [`appearance`]: font size, themes, display toggles.
//! - [`blocks`]: block fold, yank, and focus operations.
//! - [`geometry`]: chrome insets, viewports, pane hit-testing.
//! - [`init`]: GPU and window bootstrap on `resumed`.
//! - [`lifecycle`]: construction, state persistence, quit and reload.
//! - [`navigation`]: vim-style cursor motions, search, quick-select.
//! - [`notice`]: window title and transient status notices.
//! - [`palette`]: command palette and the pickers built on it.
//! - [`panes`]: pane creation, splitting, closing, per-frame upkeep.
//! - [`pointer`]: mouse hit-testing, selection, clipboard, PTY mouse forwarding.
//! - [`render`]: frame composition and WebView tile management.
//! - [`settings`]: the settings page.
//! - [`tabs`]: tab creation, activation order, the most-recently-used ring.
//! - [`window_event`]: handlers for individual winit window events.

pub mod actions;
mod appearance;
mod blocks;
mod geometry;
mod init;
mod lifecycle;
mod navigation;
mod notice;
mod palette;
mod panes;
mod pointer;
mod prompt_edit;
mod render;
mod session_restore;
mod settings;
mod tabbar;
mod tabs;
mod window_event;

use navigation::{SearchState, VimState};
use pointer::{PointerState, SelectionState};
use prompt_edit::PromptShadow;
use tabbar::MenuState;

use std::collections::{HashMap, HashSet};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey, PhysicalKey};
use winit::window::{ResizeDirection, Window, WindowId};

use crate::config::{Config, StatusBarConfig};
use crate::control::ControlMessage;
use crate::model::input::{self, Action, KeyCode, PendingPrefix, VisualKind, WindowKeymap};
use crate::model::layout::{PaneId, Tab};
use crate::model::mode::Mode;
use crate::model::palette::Palette;
use crate::model::settings_page::{ChoiceOption, SettingsPage};
use crate::terminal::pane::Pane;
use crate::terminal::webview::WebViewManager;
use winter_render::renderer::GpuRenderer;
use winter_render::{NoticeKind, StatusBar, StatusNotice, StatusSearch, TabbarHit, Theme};

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
            self.vim
                .insert_sessions
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
    pub(crate) dirty: bool,
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
    /// Previously executed palette queries, persisted across sessions.
    pub(crate) palette_history: Vec<String>,
    /// Next free globally-unique pane id; allocated by [`Self::alloc_pane_id`].
    pub(crate) next_pane_id: u64,
    /// All open tabs, each its own split-tree of panes.
    pub(crate) tabs: Vec<Tab>,
    pub(crate) modes: HashMap<PaneId, Mode>,
    pub(crate) webview_mgr: WebViewManager,
    pub(crate) window: Option<Arc<Window>>,
    /// Configurable split/close/focus key bindings (the `window` keybindings
    /// block), resolved against in Normal mode.
    pub(crate) window_keymap: WindowKeymap,
    pub(crate) window_title: String,
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
impl App {}

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
    use crate::model::layout::Rect;
    use crate::model::palette::PaletteMode;

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
        app.search.query = Some("foo".to_string());
        app.search.match_total = 2;

        app.handle_action(input::Action::SearchCancel, focused);

        assert_eq!(app.modes.get(&focused), Some(&Mode::Normal));
        assert!(app.search.query.is_none());
        assert_eq!(app.search.match_total, 0);
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

        app.search.query = Some("foo".to_string());
        assert!(app.status_bar_visible());

        app.search.query = None;
        assert!(!app.status_bar_visible());
    }

    #[test]
    fn test_status_bar_visible_stays_on_when_configured_on_regardless_of_search() {
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        assert!(app.status_bar_visible());

        app.search.query = Some("foo".to_string());
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
        app.search.query = Some("foo".to_string());
        app.search.match_index = 1;
        app.search.match_total = 3;
        assert!(app.status_bar_visible());

        // `i` back to Insert (done browsing, not a `SearchCancel` mid-input)
        // still ends the search and lets the bar drop back to hidden.
        app.handle_action(input::Action::SwitchMode(Mode::Insert), focused);
        assert!(app.search.query.is_none());
        assert_eq!(app.search.match_index, 0);
        assert_eq!(app.search.match_total, 0);
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
        app.pointer.mouse_down = true;
        app.selection.span = Some(Selection {
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
        app.pointer.cursor_pos = (10.0, 23.0);

        app.auto_scroll_selection();

        let pane = &app.panes[&id];
        assert_eq!(
            pane.grid().scroll_offset(),
            1,
            "should scroll one line into history"
        );
        let sel = app.selection.span.as_ref().expect("selection still active");
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
        app.pointer.mouse_down = true;
        let start_abs = app.panes[&id].grid().to_absolute_row(5);
        app.selection.span = Some(Selection {
            block: false,
            start_row: start_abs,
            start_col: 0,
            end_row: start_abs,
            end_col: 0,
            pane: id,
        });
        app.pointer.cursor_pos = (10.0, 23.0);

        let mut ends = Vec::new();
        for _ in 0..3 {
            app.pointer.auto_scroll_next = Instant::now(); // Bypass the interval throttle.
            app.auto_scroll_selection();
            ends.push(app.selection.span.as_ref().unwrap().end_row);
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
            app.pointer.mouse_down = true;
            app.selection.span = Some(Selection {
                block: false,
                start_row: 5,
                start_col: 0,
                end_row: 5,
                end_col: 0,
                pane: id,
            });
            app.pointer.cursor_pos = (10.0, 23.0);
            app.auto_scroll_selection();
            app.panes[&id].grid().scroll_offset()
        };
        let deep = {
            let mut app = App::new();
            let id = PaneId(1);
            app.panes.insert(id, pane_with_scrollback());
            app.pointer.mouse_down = true;
            app.selection.span = Some(Selection {
                block: false,
                start_row: 5,
                start_col: 0,
                end_row: 5,
                end_col: 0,
                pane: id,
            });
            app.pointer.cursor_pos = (10.0, 0.0);
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
        app.pointer.mouse_down = true;
        // Anchor the drag on the live view's current top row (still holding
        // real numbers, not the fresh blank row the cursor just moved to).
        let start_abs = app.panes[&id].grid().to_absolute_row(0);
        app.selection.span = Some(Selection {
            block: false,
            start_row: start_abs,
            start_col: 0,
            end_row: start_abs,
            end_col: 0,
            pane: id,
        });
        app.pointer.cursor_pos = (10.0, 23.0);

        // Scroll well past both a single page's worth of rows and the whole
        // available scrollback, so the offset saturates at the oldest line.
        for _ in 0..(rows + 25) {
            app.pointer.auto_scroll_next = Instant::now();
            app.auto_scroll_selection();
        }
        assert_eq!(
            app.panes[&id].grid().scroll_offset(),
            app.panes[&id].grid().scrollback_len(),
            "should have scrolled all the way back"
        );

        let sel = app.selection.span.as_ref().unwrap();
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
        app.pointer.mouse_down = true;
        app.selection.span = Some(Selection {
            block: false,
            start_row: 5,
            start_col: 0,
            end_row: 5,
            end_col: 0,
            pane: id,
        });
        app.pointer.cursor_pos = (10.0, 23.0);

        app.auto_scroll_selection();
        app.auto_scroll_selection();

        // The second call lands before `pointer.auto_scroll_next`, so it must not
        // advance the scrollback a second time.
        assert_eq!(app.panes[&id].grid().scroll_offset(), 1);
    }

    #[test]
    fn test_auto_scroll_selection_noop_when_pointer_not_near_an_edge() {
        let mut app = App::new();
        let id = PaneId(1);
        app.panes.insert(id, pane_with_scrollback());
        app.pointer.mouse_down = true;
        app.selection.span = Some(Selection {
            block: false,
            start_row: 5,
            start_col: 0,
            end_row: 5,
            end_col: 0,
            pane: id,
        });
        app.pointer.cursor_pos = (10.0, 300.0);

        app.auto_scroll_selection();

        assert_eq!(app.panes[&id].grid().scroll_offset(), 0);
    }

    #[test]
    fn test_auto_scroll_selection_noop_when_mouse_not_held() {
        let mut app = App::new();
        let id = PaneId(1);
        app.panes.insert(id, pane_with_scrollback());
        app.pointer.mouse_down = false;
        app.selection.span = Some(Selection {
            block: false,
            start_row: 5,
            start_col: 0,
            end_row: 5,
            end_col: 0,
            pane: id,
        });
        app.pointer.cursor_pos = (10.0, 1.0);

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
