//! KDL configuration parser: theme, fonts, keybindings, and appearance.
//!
//! Configuration lives in `~/.config/winter-term/` (or `$XDG_CONFIG_HOME/winter-term`;
//! `%APPDATA%\winter-term` on Windows) split across two files:
//! - `settings.kdl`: appearance and behavior (theme, fonts, colors, menu style,
//!   status bar).
//! - `keybindings.kdl`: keybindings, as top-level mode blocks.
//!
//! A legacy single-file `winter.kdl` (settings + a `keybindings` block) is
//! still read when neither split file is present.
//!
//! User themes live in `themes/<name>.kdl`; set `theme "<name>"` to select one
//! (the reserved words `dark`/`light`/`auto` stay built-in). A theme file is an
//! optional `base "dark"|"light"` plus a `colors` block layered over it:
//! ```kdl
//! base "dark"
//! colors {
//!     background "#282a36"
//!     foreground "#f8f8f2"
//! }
//! ```
//!
//! `settings.kdl`:
//! ```kdl
//! theme "auto"
//! font "FiraCode Nerd Font"
//! font-size "15"
//! opacity "1.0"
//! menu-style "modern"
//! window-controls-side "right"
//! window-title-template "Winter - {{ title }}"
//! cursor {
//!     insert "bar"
//!     normal "block"
//!     visual "block"
//! }
//!
//! colors {
//!     background "#2a2f31"
//!     foreground "#d8d8d8"
//!     cursor-bg "#52ad70"
//!     selection-bg "#2257a0"
//!     split "#51554f"
//!     visual-bell "#202020"
//!     ansi {
//!         black "#000000"
//!         red "#c22727"
//!     }
//!     brights {
//!         red "#d43f30"
//!     }
//!     indexed {
//!         "136" "#af8700"
//!     }
//! }
//! ```
//!
//! `keybindings.kdl` (action blocks at the top level; Normal/Insert/Visual
//! mode motions are built-in and not configurable):
//! ```kdl
//! // Window management and app-level shortcuts. The key is one or two chords;
//! // a two-chord binding sets the leader (default `C+w`). Actions:
//! // split_vertical, split_horizontal, close_pane, close_other_panes,
//! // focus_left, focus_down, focus_up, focus_right, toggle_pane_zoom,
//! // move_tab_left, move_tab_down, move_tab_up, move_tab_right,
//! // prev_tab, next_tab, new_tab, close_tab, close_other_tabs, copy_selection,
//! // paste_from_clipboard, open_settings, font_increase, font_decrease,
//! // font_reset, toggle_command_palette, toggle_history_palette,
//! // select_pane, next_block, prev_block. Pane/tab actions targeting one
//! // specific index use close_pane_N / focus_pane_N / goto_tab_N (N = 1..9):
//! // each binds one exact chord to that one pane/tab index.
//! window {
//!     "S+M+-" "split_vertical"
//!     "C+S+q" "close_pane"
//!     "C+h" "focus_left"
//!     "C+," "open_settings"
//!     "M+q" "focus_pane_1"
//! }
//! // Prompt line edits and undo/redo. One chord per binding. Actions:
//! // delete_word_backward, delete_word_forward, delete_to_line_start,
//! // delete_to_line_end (Insert mode), and prompt_undo / prompt_redo (Insert,
//! // Normal, and the palette).
//! editing {
//!     "C+Backspace" "delete_word_backward"
//!     "C+/" "prompt_undo"
//!     "C+\\" "prompt_redo"
//! }
//! ```

pub(crate) mod colors;
pub(crate) mod diagnostics;
pub(crate) mod schema;
pub(crate) mod state;
pub(crate) mod theme;
pub(crate) mod title;
pub(crate) mod watch;

use schema::{
    controls_side_as_value, controls_side_from_value, cursor_config_from_kdl, kdl_bool, kdl_string,
    parse_keys, KdlConfig,
};

pub use colors::ColorOverrides;

// Keep the flat `crate::config::<name>` paths the rest of the app already uses.
pub(crate) use diagnostics::config_problems;
pub(crate) use state::{config_dir, load_state, save_state, AppState};
pub(crate) use theme::{available_themes, is_valid_theme_name, load_named_theme, save_named_theme};
pub(crate) use title::{
    expand_window_title_template, WindowTitleVars, DEFAULT_WINDOW_TITLE_TEMPLATE,
};

use std::collections::HashMap;

use winter_render::{ControlsSide, CursorShape, MenuStyle};

use crate::model::paint::GutterSide;

// ========================================================================
// Constants
// ========================================================================

/// The font size, in points, when `settings.kdl` names none.
const DEFAULT_FONT_SIZE: f32 = 15.0;

// ========================================================================
// Data Structures
// ========================================================================

/// Status bar visibility and content. `enabled` toggles the whole bar (which
/// also frees its reserved row); `show_mode` toggles the mode indicator;
/// `segments` picks what its right side shows, and in which order.
#[derive(Clone, Debug)]
pub struct StatusBarConfig {
    /// Whether the status bar is drawn at all.
    pub enabled: bool,
    /// The segments in the order they hold on when the bar is short of
    /// room, the first kept longest; one left out goes before any listed.
    /// Empty to drop from the left.
    pub priority: Vec<StatusSegmentKind>,
    /// The right side's segments, in order from the left.
    pub segments: Vec<StatusSegmentKind>,
    /// Whether the current mode is named in the bar.
    pub show_mode: bool,
}

impl Default for StatusBarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            priority: Vec::new(),
            segments: StatusSegmentKind::ALL.to_vec(),
            show_mode: true,
        }
    }
}

/// A segment the status bar's right side can show, named in
/// `status-bar { segments ... }`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusSegmentKind {
    /// Antigravity's quota, under its logo.
    Antigravity,
    /// Claude Code's quota, under its logo.
    Claude,
    /// What the editor that has the keyboard says about where its keys stand
    /// (`12:5`, `340 lines`), then its icon.
    Editor,
    /// Z.ai's quota, under its logo.
    Zai,
}

impl StatusSegmentKind {
    /// Every segment, in the order the right side shows them when
    /// `settings.kdl` names none.
    pub const ALL: [Self; 4] = [Self::Antigravity, Self::Claude, Self::Zai, Self::Editor];

    /// The segment `settings.kdl` calls `name`, if it is one.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "antigravity" => Some(Self::Antigravity),
            "claude" => Some(Self::Claude),
            "tool" => Some(Self::Editor),
            "zai" => Some(Self::Zai),
            _ => None,
        }
    }

    /// What `settings.kdl` calls the segment.
    pub fn name(self) -> &'static str {
        match self {
            Self::Antigravity => "antigravity",
            Self::Claude => "claude",
            Self::Editor => "tool",
            Self::Zai => "zai",
        }
    }
}

/// How notices are shown. With `toasts`, warnings and errors are cards in the
/// window's top-right corner and confirmations sit in the status bar; without,
/// all of them sit in the status bar. While the bar is hidden every notice is
/// a card, having nowhere else to go.
#[derive(Clone, Debug)]
pub struct NotificationsConfig {
    /// Whether warnings and errors are shown as toast cards.
    pub toasts: bool,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self { toasts: true }
    }
}

/// Everything the configuration files can set.
#[derive(Clone, Debug)]
pub struct Config {
    /// Let programs read the clipboard through OSC 52 (`ESC ] 52 ; c ; ?`).
    /// Opt-in because the query is silent and unconditional on the querying
    /// side: any program in the pane (including one behind ssh) could pull
    /// the clipboard's contents. Responses are capped (64 KiB of text).
    /// Default: `false`.
    pub clipboard_read: bool,
    /// Per-color overrides layered over the selected theme.
    pub colors: ColorOverrides,
    /// Cursor shape for each interaction mode.
    pub cursor: CursorConfig,
    /// Dim inactive panes by blending their colors toward the background. Default: `false`.
    pub dim_inactive: bool,
    /// Font family name; the system default is used when absent.
    pub font_family: Option<String>,
    /// Font size in points.
    pub font_size: f32,
    /// Weight used for regular text.
    pub font_weight: Option<String>,
    /// Weight used for bold text.
    pub font_weight_bold: Option<String>,
    /// Which edge of an editor's rows its gutter (the git states of a listing,
    /// its marks) stands at. Default: the left.
    pub gutter_side: GutterSide,
    /// User keybindings, keyed by mode and then by chord.
    pub keybindings: HashMap<String, HashMap<String, String>>,
    /// Why `keybindings.kdl` was ignored, when it did not parse, with the
    /// line of its first problem. Kept apart from a rejected settings.kdl: the
    /// settings themselves still apply. While it is set, [`Self::keybindings`]
    /// holds only what `settings.kdl` binds, not the file's bindings, so a
    /// reload should keep the keymap already in force rather than rebuild it
    /// from these.
    pub keybindings_error: Option<String>,
    /// Enable OpenType ligatures in the font renderer. Defaults to `false`:
    /// like most terminals, winter keeps each cell's glyph at its nominal
    /// position so contextual shaping never breaks the monospace grid (e.g.
    /// raising the middle asterisk of `***`). Set `ligatures #true` to opt in.
    pub ligatures: bool,
    /// Tabbar menu presentation: a modern hamburger dropdown or a classic menubar.
    pub menu_style: MenuStyle,
    /// How notices are shown.
    pub notifications: NotificationsConfig,
    /// Window opacity, from 0.0 (transparent) to 1.0 (opaque).
    pub opacity: f32,
    /// Draw an underline under fuzzy-matched characters in the command palette.
    pub palette_match_underline: bool,
    /// Thickness of the line drawn between adjacent panes, in logical pixels. Default: 2.
    pub pane_border_width: f32,
    /// Right-click pastes from the clipboard instead of opening the context menu.
    pub paste_on_right_click: bool,
    /// Which line-editor bindings the shell answers to, for Vim operators on
    /// the prompt line. Default: [`PromptEditBindings::Emacs`].
    pub prompt_edit_bindings: PromptEditBindings,
    /// Bracket glyphs depth-colored by nesting depth. Default: `false`.
    pub rainbow_parens: bool,
    /// Save the tab/pane layout on exit and reopen it on the next launch. Default: `false`.
    pub restore_session: bool,
    /// Maximum scrollback rows per pane. `None` uses the compiled-in default (10 000).
    pub scrollback_lines: Option<usize>,
    /// Alternating background bands per sentence. Default: `false`.
    pub sentence_highlight: bool,
    /// Shell to launch, overriding the platform-specific settings below.
    pub shell: Option<String>,
    /// Shell to launch on Windows.
    pub shell_windows: Option<String>,
    /// Shell to launch on Linux.
    pub shell_linux: Option<String>,
    /// Shell to launch on macOS.
    pub shell_macos: Option<String>,
    /// Status bar appearance.
    pub status_bar: StatusBarConfig,
    /// Which theme to load.
    pub theme: ThemeSetting,
    /// How the window's title bar is drawn.
    pub title_bar_style: TitleBarStyle,
    /// Underline `http://`/`https://` URLs (auto-detected and OSC 8
    /// hyperlinks alike). Default: `false`.
    pub url_underline: bool,
    /// Which edge carries the minimize/maximize/close buttons (the hamburger
    /// button sits on the opposite edge).
    pub window_controls_side: ControlsSide,
    /// Window title template. `{{ ... }}` placeholders expand to the running
    /// program's title/name/cwd (see [`WindowTitleVars`]); the rest is literal
    /// text. Default: `Winter - {{ title }}`.
    pub window_title_template: String,
    /// Indent soft-wrapped continuation lines to match the logical line's
    /// first non-blank column, in the scrollback once a width change
    /// settles (see `wrap_words`). Default: `true`.
    pub wrap_indent: bool,
    /// Break soft wraps at word boundaries: the partial word after the last
    /// space on a full row moves down with the wrap, instead of the row
    /// breaking exactly at the margin mid-word. Applied to the scrollback
    /// once a width change settles; the visible screen always wraps at the
    /// margin, where a program redrawing it expects. Default: `true`.
    pub wrap_words: bool,
}

/// Which line-editor bindings Winter assumes the shell uses when it realizes a
/// Vim prompt-line operator.
///
/// Normal-mode operators on the prompt line are sent to the shell as the
/// equivalent line-editor keystrokes, so Winter has to know which set the shell
/// answers to. It cannot detect this: the shell never reports its editing mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PromptEditBindings {
    /// Readline/ZLE emacs bindings (`C-a`, `C-k`, `C-u`, `C-w`,
    /// `M-d`), which is every common shell's default.
    #[default]
    Emacs,
    /// Do not translate. For a shell already in vi mode (`bindkey -v`,
    /// `set editing-mode vi`, fish's vi bindings), where those chords mean
    /// something else and the shell provides its own Vim editing anyway.
    None,
}

/// Per-mode cursor shapes, parsed from the `cursor` block. Each mode renders
/// its cursor with the configured shape; defaults follow the common terminal
/// convention (a bar for insert-like modes, a block for navigation).
#[derive(Clone, Copy, Debug)]
pub struct CursorConfig {
    /// Whether the cursor blinks. Applies only to the focused pane.
    pub blink: bool,
    /// Hide the cursor entirely in inactive (non-focused) panes, instead of
    /// showing it as a hollow outline. Default: `false`.
    pub hide_in_inactive: bool,
    /// Cursor shape in Insert mode.
    pub insert: CursorShape,
    /// Cursor shape in Normal mode.
    pub normal: CursorShape,
    /// Cursor shape in Visual mode.
    pub visual: CursorShape,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            blink: true,
            hide_in_inactive: false,
            insert: CursorShape::Bar,
            normal: CursorShape::Block,
            visual: CursorShape::Block,
        }
    }
}

/// Window title-bar presentation: the OS-drawn decorations, or a borderless
/// window whose tab strip doubles as a VS Code-style title bar carrying its own
/// minimize/maximize/close controls.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TitleBarStyle {
    /// Native OS window decorations; the tab strip sits inside, below them.
    System,
    /// Borderless: the tab strip is the title bar, with custom window controls.
    #[default]
    Modern,
}

impl TitleBarStyle {
    /// Interpret a `title-bar-style` config value; unknown values fall back to the
    /// modern default. `native` is accepted as an alias for `system`.
    pub fn from_value(value: &str) -> Self {
        match value {
            "system" | "native" => Self::System,
            _ => Self::Modern,
        }
    }

    /// The `title-bar-style` config value this selection serializes to.
    pub fn as_value(&self) -> &str {
        match self {
            Self::System => "system",
            Self::Modern => "modern",
        }
    }
}

/// The selected theme: a built-in preset, the system-following `Auto`, or a
/// user theme file `themes/<name>.kdl` referenced by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ThemeSetting {
    /// Follow the system light or dark preference.
    Auto,
    /// Always use the built-in dark theme.
    #[default]
    Dark,
    /// Always use the built-in light theme.
    Light,
    /// Load the named theme file from the themes directory.
    Named(String),
}

impl ThemeSetting {
    /// Interpret a `theme` config value: the reserved words `auto`/`dark`/`light`
    /// are built-ins, anything else names a theme file in `themes/`.
    pub fn from_value(value: &str) -> Self {
        match value {
            "auto" => Self::Auto,
            "light" => Self::Light,
            "dark" => Self::Dark,
            other => Self::Named(other.to_string()),
        }
    }

    /// The `theme` config value this selection serializes to.
    pub fn as_value(&self) -> &str {
        match self {
            Self::Auto => "auto",
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Named(name) => name,
        }
    }
}

// ========================================================================
// KDL schema (KDL v2, deserialized via serde)
// ========================================================================

// ========================================================================
// Implementation
// ========================================================================

impl Config {
    /// Returns the configured shell path/name specifically active for the current target OS,
    /// falling back to the generic `shell` option if not set.
    pub fn active_shell(&self) -> Option<&str> {
        #[cfg(target_os = "windows")]
        {
            self.shell_windows.as_deref().or(self.shell.as_deref())
        }
        #[cfg(target_os = "macos")]
        {
            self.shell_macos.as_deref().or(self.shell.as_deref())
        }
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            self.shell_linux.as_deref().or(self.shell.as_deref())
        }
    }
    /// Load configuration from `~/.config/winter-term/`, also returning a
    /// one-line summary when a config file was rejected.
    ///
    /// Settings come from `settings.kdl` and keybindings from
    /// `keybindings.kdl`. If neither exists, falls back to the legacy
    /// single-file `winter.kdl`; failing that, defaults.
    ///
    /// The diagnostic is returned rather than only logged because the GUI is
    /// usually launched from a desktop menu with no terminal attached, so a
    /// message on stderr is invisible and the user just sees their settings
    /// not applying.
    pub fn load_checked() -> (Self, Option<String>) {
        let dir = config_dir();
        let settings_path = dir.join("settings.kdl");
        let keys_path = dir.join("keybindings.kdl");

        if settings_path.exists() || keys_path.exists() {
            let settings = std::fs::read_to_string(&settings_path).unwrap_or_default();
            let keys = std::fs::read_to_string(&keys_path).unwrap_or_default();
            return Self::parse_with_keys_checked(&settings, &keys);
        }

        let legacy = dir.join("winter.kdl");
        if legacy.exists() {
            match std::fs::read_to_string(&legacy) {
                Ok(text) => Self::parse_checked(&text),
                Err(_) => (Self::default(), None),
            }
        } else {
            (Self::default(), None)
        }
    }

    /// Parse settings (`settings.kdl`) and keybindings (`keybindings.kdl`) from separate
    /// sources. Keybindings in `keys` replace any present in `settings`.
    pub fn parse_with_keys(settings: &str, keys: &str) -> Self {
        Self::parse_with_keys_checked(settings, keys).0
    }

    /// [`Self::parse_with_keys`], also returning a one-line summary when the
    /// settings file was rejected.
    pub fn parse_with_keys_checked(settings: &str, keys: &str) -> (Self, Option<String>) {
        let (mut config, error) = Self::parse_checked(settings);
        match parse_keys(keys) {
            Ok(parsed) if !parsed.is_empty() => config.keybindings = parsed,
            Ok(_) => {}
            Err(problem) => {
                config.keybindings_error = Some(format!("keybindings.kdl ignored: {problem}"));
            }
        }
        (config, error)
    }

    /// Parse configuration text, falling back to defaults for anything malformed.
    pub fn parse(text: &str) -> Self {
        Self::parse_checked(text).0
    }

    /// [`Self::parse`], also returning a one-line summary when the file was
    /// rejected and defaults were substituted.
    ///
    /// The GUI is usually launched from a desktop menu with no terminal
    /// attached, so a diagnostic written only to stderr is invisible: the user
    /// sees their settings silently not applying. The caller surfaces this in
    /// the status bar instead.
    pub fn parse_checked(text: &str) -> (Self, Option<String>) {
        let kdl: KdlConfig = match kdl::de::from_str(text) {
            Ok(c) => c,
            Err(e) => {
                let problems = config_problems(&e, text);
                eprintln!(
                    "winter: ignoring settings.kdl ({} problem(s)); using built-in defaults until fixed:",
                    problems.len()
                );
                for problem in &problems {
                    eprintln!("  line {}: {}", problem.line, problem.message);
                }
                let summary = match problems.first() {
                    Some(first) => format!(
                        "settings.kdl ignored ({} problem(s)); line {}: {}",
                        problems.len(),
                        first.line,
                        first.message
                    ),
                    None => "settings.kdl ignored; using built-in defaults".to_string(),
                };
                return (Self::default(), Some(summary));
            }
        };

        let theme = kdl
            .theme
            .as_deref()
            .map(ThemeSetting::from_value)
            .unwrap_or_default();

        let keybindings = kdl.keybindings.unwrap_or_default();

        let mut unknown_segments = Vec::new();
        let status_bar = kdl
            .status_bar
            .map(|sb| StatusBarConfig {
                enabled: sb.show.unwrap_or(true),
                priority: sb
                    .priority
                    .map(|names| status_segments_from_names(names, &mut unknown_segments))
                    .unwrap_or_default(),
                segments: sb.segments.map_or_else(
                    || StatusSegmentKind::ALL.to_vec(),
                    |names| status_segments_from_names(names, &mut unknown_segments),
                ),
                show_mode: sb.show_mode.unwrap_or(true),
            })
            .unwrap_or_default();

        let menu_style = kdl
            .menu_style
            .as_deref()
            .map(|s| match s {
                "classic" => MenuStyle::Classic,
                _ => MenuStyle::Modern,
            })
            .unwrap_or_default();

        let title_bar_style = kdl
            .title_bar_style
            .as_deref()
            .map(TitleBarStyle::from_value)
            .unwrap_or_default();

        let window_controls_side = kdl
            .window_controls_side
            .as_deref()
            .map(controls_side_from_value)
            .unwrap_or_default();

        let config = Config {
            colors: kdl.colors.map(ColorOverrides::from_kdl).unwrap_or_default(),
            cursor: kdl.cursor.map(cursor_config_from_kdl).unwrap_or_default(),
            dim_inactive: kdl.dim_inactive.unwrap_or(false),
            font_family: kdl.font.filter(|s| !s.trim().is_empty()),
            font_size: kdl
                .font_size
                .map_or(DEFAULT_FONT_SIZE, winter_render::clamp_font_size),
            font_weight: kdl.font_weight.filter(|s| !s.trim().is_empty()),
            font_weight_bold: kdl.font_weight_bold.filter(|s| !s.trim().is_empty()),
            gutter_side: kdl
                .gutter_side
                .as_deref()
                .map(GutterSide::from_value)
                .unwrap_or_default(),
            keybindings,
            keybindings_error: None,
            menu_style,
            notifications: kdl
                .notifications
                .map(|block| NotificationsConfig {
                    toasts: block.toasts.unwrap_or(true),
                })
                .unwrap_or_default(),
            opacity: kdl.opacity.unwrap_or(1.0).clamp(0.1, 1.0),
            ligatures: kdl.ligatures.unwrap_or(false),
            clipboard_read: kdl.clipboard_read.unwrap_or(false),
            palette_match_underline: kdl.palette_match_underline.unwrap_or(false),
            pane_border_width: kdl.pane_border_width.map(|w| w.max(1.0)).unwrap_or(1.0),
            paste_on_right_click: kdl.paste_on_right_click.unwrap_or(false),
            prompt_edit_bindings: kdl
                .prompt_edit_bindings
                .as_deref()
                .map(PromptEditBindings::from_value)
                .unwrap_or_default(),
            rainbow_parens: kdl.rainbow_parens.unwrap_or(false),
            restore_session: kdl.restore_session.unwrap_or(false),
            scrollback_lines: kdl.scrollback_lines.map(|n| n as usize).filter(|&n| n > 0),
            sentence_highlight: kdl.sentence_highlight.unwrap_or(false),
            shell: kdl.shell.filter(|s| !s.trim().is_empty()),
            shell_windows: kdl.shell_windows.filter(|s| !s.trim().is_empty()),
            shell_linux: kdl.shell_linux.filter(|s| !s.trim().is_empty()),
            shell_macos: kdl.shell_macos.filter(|s| !s.trim().is_empty()),
            status_bar,
            theme,
            title_bar_style,
            url_underline: kdl.url_underline.unwrap_or(false),
            window_controls_side,
            window_title_template: kdl
                .window_title_template
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_WINDOW_TITLE_TEMPLATE.to_string()),
            wrap_indent: kdl.wrap_indent.unwrap_or(true),
            wrap_words: kdl.wrap_words.unwrap_or(true),
        };
        let warning = unknown_syntax_warning(&config.colors.unknown_syntax)
            .or_else(|| unknown_segment_warning(&unknown_segments));
        (config, warning)
    }

    /// Serialize the appearance/behavior settings as `settings.kdl` text. The
    /// output round-trips through [`Self::parse`]. Keybindings live in a separate
    /// `keybindings.kdl` and are intentionally not written here.
    pub fn to_kdl(&self) -> String {
        let menu_style = match self.menu_style {
            MenuStyle::Classic => "classic",
            MenuStyle::Modern => "modern",
        };

        let mut out = String::new();
        out.push_str(&format!("theme {}\n", kdl_string(self.theme.as_value())));
        if let Some(n) = self.scrollback_lines {
            out.push_str(&format!("scrollback-lines {n}\n"));
        }
        if let Some(shell) = &self.shell {
            out.push_str(&format!("shell {}\n", kdl_string(shell)));
        }
        if let Some(shell) = &self.shell_windows {
            out.push_str(&format!("shell-windows {}\n", kdl_string(shell)));
        }
        if let Some(shell) = &self.shell_linux {
            out.push_str(&format!("shell-linux {}\n", kdl_string(shell)));
        }
        if let Some(shell) = &self.shell_macos {
            out.push_str(&format!("shell-macos {}\n", kdl_string(shell)));
        }
        if let Some(font) = &self.font_family {
            out.push_str(&format!("font {}\n", kdl_string(font)));
        }
        out.push_str(&format!("font-size {}\n", self.font_size));
        if let Some(font_weight) = &self.font_weight {
            out.push_str(&format!("font-weight {}\n", kdl_string(font_weight)));
        }
        if let Some(bold_weight) = &self.font_weight_bold {
            out.push_str(&format!("font-weight-bold {}\n", kdl_string(bold_weight)));
        }
        out.push_str(&format!("opacity {}\n", self.opacity));
        out.push_str(&format!("menu-style {}\n", kdl_string(menu_style)));
        out.push_str(&format!(
            "palette-match-underline {}\n",
            kdl_bool(self.palette_match_underline)
        ));
        out.push_str(&format!("pane-border-width {}\n", self.pane_border_width));
        out.push_str(&format!("ligatures {}\n", kdl_bool(self.ligatures)));
        out.push_str(&format!(
            "clipboard-read {}\n",
            kdl_bool(self.clipboard_read)
        ));
        out.push_str(&format!(
            "paste-on-right-click {}\n",
            kdl_bool(self.paste_on_right_click)
        ));
        out.push_str(&format!(
            "prompt-edit-bindings {}\n",
            kdl_string(self.prompt_edit_bindings.as_value())
        ));
        out.push_str(&format!(
            "rainbow-parens {}\n",
            kdl_bool(self.rainbow_parens)
        ));
        out.push_str(&format!(
            "sentence-highlight {}\n",
            kdl_bool(self.sentence_highlight)
        ));
        out.push_str(&format!("url-underline {}\n", kdl_bool(self.url_underline)));
        out.push_str(&format!("wrap-indent {}\n", kdl_bool(self.wrap_indent)));
        out.push_str(&format!("wrap-words {}\n", kdl_bool(self.wrap_words)));
        out.push_str(&format!("dim-inactive {}\n", kdl_bool(self.dim_inactive)));
        out.push_str(&format!(
            "restore-session {}\n",
            kdl_bool(self.restore_session)
        ));
        out.push_str(&format!(
            "title-bar-style {}\n",
            kdl_string(self.title_bar_style.as_value())
        ));
        out.push_str(&format!(
            "gutter-side {}\n",
            kdl_string(self.gutter_side.as_value())
        ));
        out.push_str(&format!(
            "window-controls-side {}\n",
            kdl_string(controls_side_as_value(self.window_controls_side))
        ));
        out.push_str(&format!(
            "window-title-template {}\n",
            kdl_string(&self.window_title_template)
        ));
        out.push_str(&self.cursor_kdl());
        out.push_str(&self.status_bar_kdl());
        out.push_str(&self.notifications_kdl());
        if let Some(colors) = self.colors.to_kdl() {
            out.push_str(&colors);
        }
        out
    }

    /// Write [`Self::to_kdl`] to `settings.kdl`, creating the config directory if
    /// needed. Keybindings are left untouched.
    pub fn save(&self) -> std::io::Result<()> {
        crate::persist::write_atomic(&config_dir().join("settings.kdl"), self.to_kdl())
    }

    /// The `cursor` block, always emitted so every per-mode shape round-trips.
    fn cursor_kdl(&self) -> String {
        let c = &self.cursor;
        let mut out = String::from("cursor {\n");
        out.push_str(&format!("    blink {}\n", kdl_bool(c.blink)));
        out.push_str(&format!("    insert {}\n", kdl_string(c.insert.as_value())));
        out.push_str(&format!("    normal {}\n", kdl_string(c.normal.as_value())));
        out.push_str(&format!("    visual {}\n", kdl_string(c.visual.as_value())));
        out.push_str(&format!(
            "    hide-in-inactive {}\n",
            kdl_bool(c.hide_in_inactive)
        ));
        out.push_str("}\n");
        out
    }

    /// The `notifications` block, always emitted so every flag round-trips.
    fn notifications_kdl(&self) -> String {
        format!(
            "notifications {{\n    toasts {}\n}}\n",
            kdl_bool(self.notifications.toasts)
        )
    }

    /// The `status-bar` block, always emitted so every flag round-trips.
    fn status_bar_kdl(&self) -> String {
        let sb = &self.status_bar;
        let mut out = String::from("status-bar {\n");
        out.push_str(&format!("    show {}\n", kdl_bool(sb.enabled)));
        out.push_str(&format!("    show-mode {}\n", kdl_bool(sb.show_mode)));
        out.push_str("    segments");
        for segment in &sb.segments {
            out.push_str(&format!(" {}", kdl_string(segment.name())));
        }
        out.push('\n');
        // Left out when unset, since a bare `priority` would read as an
        // empty list, which is the same as none.
        if !sb.priority.is_empty() {
            out.push_str("    priority");
            for segment in &sb.priority {
                out.push_str(&format!(" {}", kdl_string(segment.name())));
            }
            out.push('\n');
        }
        out.push_str("}\n");
        out
    }
}

impl PromptEditBindings {
    /// Parse the `prompt-edit-bindings` value. An unrecognized word keeps the
    /// default rather than silently disabling prompt editing.
    pub(crate) fn from_value(value: &str) -> Self {
        match value {
            "none" | "off" | "vi" => PromptEditBindings::None,
            _ => PromptEditBindings::Emacs,
        }
    }

    /// The canonical spelling written back by [`Config::to_kdl`].
    fn as_value(self) -> &'static str {
        match self {
            PromptEditBindings::Emacs => "emacs",
            PromptEditBindings::None => "none",
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            colors: ColorOverrides::default(),
            cursor: CursorConfig::default(),
            dim_inactive: false,
            font_family: None,
            font_size: DEFAULT_FONT_SIZE,
            font_weight: None,
            font_weight_bold: None,
            gutter_side: GutterSide::default(),
            keybindings: HashMap::new(),
            keybindings_error: None,
            ligatures: false,
            clipboard_read: false,
            menu_style: MenuStyle::default(),
            notifications: NotificationsConfig::default(),
            opacity: 1.0,
            palette_match_underline: false,
            pane_border_width: 1.0,
            paste_on_right_click: false,
            prompt_edit_bindings: PromptEditBindings::default(),
            rainbow_parens: false,
            restore_session: false,
            scrollback_lines: None,
            sentence_highlight: false,
            shell: None,
            shell_windows: None,
            shell_linux: None,
            shell_macos: None,
            status_bar: StatusBarConfig::default(),
            theme: ThemeSetting::default(),
            title_bar_style: TitleBarStyle::default(),
            url_underline: false,
            window_controls_side: ControlsSide::default(),
            window_title_template: DEFAULT_WINDOW_TITLE_TEMPLATE.to_string(),
            wrap_indent: true,
            wrap_words: true,
        }
    }
}

// ========================================================================
// Window title template
// ========================================================================

// ========================================================================
// Functions
// ========================================================================

/// What to say about `syntax` colors named by no scope of the theme's table,
/// which would otherwise color nothing without a word. `Editor: Show Scopes
/// at Cursor` names the scope a character is in.
fn unknown_syntax_warning(names: &[String]) -> Option<String> {
    let first = names.first()?;
    let more = match names.len() {
        1 => String::new(),
        n => format!(" and {} more", n - 1),
    };
    Some(format!(
        "settings.kdl: no syntax scope named {first}{more}; \
         Text Editor: Show Scopes at Cursor names them"
    ))
}

/// The segments `names` call for, in their order; a name that is none is
/// left out and added to `unknown`.
fn status_segments_from_names(
    names: Vec<String>,
    unknown: &mut Vec<String>,
) -> Vec<StatusSegmentKind> {
    names
        .into_iter()
        .filter_map(|name| {
            let kind = StatusSegmentKind::from_name(&name);
            if kind.is_none() {
                unknown.push(name);
            }
            kind
        })
        .collect()
}

/// What to say about `status-bar { segments ... }` names that are no
/// segment, which would otherwise show nothing without a word.
fn unknown_segment_warning(names: &[String]) -> Option<String> {
    let first = names.first()?;
    let known: Vec<&str> = StatusSegmentKind::ALL
        .iter()
        .map(|kind| kind.name())
        .collect();
    Some(format!(
        "settings.kdl: no status-bar segment named {first}; the segments are {}",
        known.join(", ")
    ))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    // Tests build configs by mutating fields off `default()` for readability,
    // which is clearer here than large struct literals.
    #![allow(clippy::field_reassign_with_default)]

    use winter_render::{Theme, ThemeRgb};

    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.font_size, 15.0);
        assert_eq!(config.opacity, 1.0);
        assert_eq!(config.theme, ThemeSetting::Dark);
        assert!(config.keybindings.is_empty());
    }

    #[test]
    fn test_parse_minimal_config() {
        let config = Config::parse("font-size 18\nopacity 0.9");
        assert_eq!(config.font_size, 18.0);
        assert_eq!(config.opacity, 0.9);
    }

    #[test]
    fn test_valid_config_has_no_problems() {
        assert!(kdl::de::from_str::<KdlConfig>("theme \"dark\"\n").is_ok());
    }

    #[test]
    fn test_parse_font_family() {
        let config = Config::parse("font \"FiraCode Nerd Font\"");
        assert_eq!(config.font_family.as_deref(), Some("FiraCode Nerd Font"));
    }

    #[test]
    fn test_default_font_family_is_none() {
        assert_eq!(Config::default().font_family, None);
        assert_eq!(Config::parse("font-size 12").font_family, None);
    }

    #[test]
    fn test_parse_scrollback_lines() {
        let config = Config::parse("scrollback-lines 50000");
        assert_eq!(config.scrollback_lines, Some(50_000));
    }

    #[test]
    fn test_scrollback_lines_zero_is_ignored() {
        let config = Config::parse("scrollback-lines 0");
        assert_eq!(config.scrollback_lines, None);
    }

    #[test]
    fn test_to_kdl_roundtrips_scrollback_lines() {
        let mut config = Config::default();
        config.scrollback_lines = Some(20_000);
        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.scrollback_lines, Some(20_000));
    }

    #[test]
    fn test_parse_shell() {
        let config = Config::parse("shell \"/usr/bin/fish\"");
        assert_eq!(config.shell.as_deref(), Some("/usr/bin/fish"));
    }

    #[test]
    fn test_default_shell_is_none() {
        assert_eq!(Config::default().shell, None);
        assert_eq!(Config::parse("font-size 12").shell, None);
    }

    #[test]
    fn test_shell_whitespace_ignored() {
        let config = Config::parse("shell \"   \"");
        assert_eq!(config.shell, None);
    }

    #[test]
    fn test_parse_colors_block() {
        let config = Config::parse(
            r##"
colors {
    background "#1a1a2e"
    foreground "#e0e0e0"
    cursor-bg "#4fa2fa"
    scrollbar "#2f6099"
    split "#333333"
    window-border "#414248"
    ansi {
        black "#000000"
        red "#c22727"
    }
    indexed {
        "136" "#af8700"
    }
}
"##,
        );
        let mut theme = Theme::dark();
        config.colors.apply(&mut theme);
        assert_eq!(theme.background, ThemeRgb::parse_hex("#1a1a2e").unwrap());
        assert_eq!(theme.foreground, ThemeRgb::parse_hex("#e0e0e0").unwrap());
        assert_eq!(theme.divider, ThemeRgb::parse_hex("#333333").unwrap());
        assert_eq!(theme.window_border, ThemeRgb::parse_hex("#414248").unwrap());
        assert_eq!(theme.ansi[1], ThemeRgb::parse_hex("#c22727").unwrap());
        assert_eq!(theme.indexed_color(136), Some((175, 135, 0)));
        // cursor and scrollbar are overridden to the configured blues.
        assert_eq!(theme.cursor_bg, ThemeRgb::parse_hex("#4fa2fa").unwrap());
        assert_eq!(theme.scrollbar, ThemeRgb::parse_hex("#2f6099").unwrap());
    }

    #[test]
    fn test_no_colors_block_leaves_preset_untouched() {
        let config = Config::parse("theme \"dark\"");
        let mut theme = Theme::dark();
        config.colors.apply(&mut theme);
        assert_eq!(theme, Theme::dark());
    }

    #[test]
    fn test_window_title_template_defaults_and_roundtrips() {
        assert_eq!(
            Config::default().window_title_template,
            "Winter - {{ title }}".to_string()
        );
        assert_eq!(
            Config::parse("window-title-template \"{{ app_name }} — {{ pane_title }}\"")
                .window_title_template,
            "{{ app_name }} — {{ pane_title }}".to_string()
        );
        // Unset or empty falls back to the default.
        assert_eq!(
            Config::parse("").window_title_template,
            "Winter - {{ title }}".to_string()
        );
        assert_eq!(
            Config::parse("window-title-template \"\"").window_title_template,
            "Winter - {{ title }}".to_string()
        );
        // Serializes and parses back to the same template.
        let mut config = Config::default();
        config.window_title_template = "{{ app_name }}".to_string();
        assert_eq!(
            Config::parse(&config.to_kdl()).window_title_template,
            "{{ app_name }}".to_string()
        );
    }

    #[test]
    fn test_title_bar_style_defaults_modern_and_roundtrips() {
        assert_eq!(Config::default().title_bar_style, TitleBarStyle::Modern);
        assert_eq!(
            Config::parse("title-bar-style \"system\"").title_bar_style,
            TitleBarStyle::System
        );
        // `native` is an alias; unknown values fall back to the modern default.
        assert_eq!(
            Config::parse("title-bar-style \"native\"").title_bar_style,
            TitleBarStyle::System
        );
        assert_eq!(
            Config::parse("title-bar-style \"bogus\"").title_bar_style,
            TitleBarStyle::Modern
        );
        // Serializes and parses back to the same selection.
        let mut config = Config::default();
        config.title_bar_style = TitleBarStyle::System;
        assert_eq!(
            Config::parse(&config.to_kdl()).title_bar_style,
            TitleBarStyle::System
        );
    }

    #[test]
    fn test_parse_theme_auto() {
        let config = Config::parse("theme \"auto\"");
        assert_eq!(config.theme, ThemeSetting::Auto);
    }

    #[test]
    fn test_parse_theme_light() {
        let config = Config::parse("theme \"light\"");
        assert_eq!(config.theme, ThemeSetting::Light);
    }

    #[test]
    fn test_parse_keybindings() {
        let config = Config::parse(
            r#"
keybindings {
    normal {
        j "focus_down"
        k "focus_up"
    }
    insert {
        "C-SPC" "toggle_mode"
    }
}
"#,
        );
        assert_eq!(config.keybindings.len(), 2);
        let normal = config.keybindings.get("normal").unwrap();
        assert_eq!(normal.get("j"), Some(&"focus_down".to_string()));
        assert_eq!(normal.get("k"), Some(&"focus_up".to_string()));
        let insert = config.keybindings.get("insert").unwrap();
        assert_eq!(insert.get("C-SPC"), Some(&"toggle_mode".to_string()));
    }

    #[test]
    fn test_opacity_clamped() {
        let config = Config::parse("opacity 0.05");
        assert_eq!(config.opacity, 0.1);
    }

    #[test]
    fn test_parse_invalid_returns_default() {
        let config = Config::parse("this is not valid kdl {{{{");
        assert_eq!(config.font_size, 15.0);
    }

    #[test]
    fn test_parse_with_keys_merges_separate_files() {
        let settings = "theme \"light\"\nmenu-style \"classic\"";
        let keys = r#"
normal {
    j "focus_down"
}
window {
    "C-w v" "split_vertical"
}
"#;
        let config = Config::parse_with_keys(settings, keys);
        // Settings come from settings.kdl.
        assert_eq!(config.theme, ThemeSetting::Light);
        assert_eq!(config.menu_style, MenuStyle::Classic);
        // Keybindings come from keybindings.kdl (top-level mode blocks).
        assert_eq!(
            config.keybindings.get("normal").and_then(|m| m.get("j")),
            Some(&"focus_down".to_string())
        );
        assert_eq!(
            config
                .keybindings
                .get("window")
                .and_then(|m| m.get("C-w v")),
            Some(&"split_vertical".to_string())
        );
    }

    #[test]
    fn test_parse_with_empty_keys_keeps_no_bindings() {
        let config = Config::parse_with_keys("font-size 12", "");
        assert!(config.keybindings.is_empty());
    }

    #[test]
    fn test_menu_style_defaults_to_modern_and_parses_classic() {
        assert_eq!(Config::default().menu_style, MenuStyle::Modern);
        assert_eq!(Config::parse("font-size 12").menu_style, MenuStyle::Modern);
        assert_eq!(
            Config::parse("menu-style \"classic\"").menu_style,
            MenuStyle::Classic
        );
        // An unrecognized value falls back to the modern default.
        assert_eq!(
            Config::parse("menu-style \"fancy\"").menu_style,
            MenuStyle::Modern
        );
    }

    #[test]
    fn test_parse_cursor_block_and_synonyms() {
        let config = Config::parse(
            r#"
cursor {
    insert "beam"
    normal "underline"
    visual "underscore"
    block-focus "bar"
}
"#,
        );
        // Synonyms resolve to the canonical variants; "beam"→Bar,
        // "underline"/"underscore"→Underline.
        assert_eq!(config.cursor.insert, CursorShape::Bar);
        assert_eq!(config.cursor.normal, CursorShape::Underline);
        assert_eq!(config.cursor.visual, CursorShape::Underline);
    }

    #[test]
    fn test_window_controls_side_defaults_left_parses_right() {
        assert_eq!(Config::default().window_controls_side, ControlsSide::Left);

        let config = Config::parse("window-controls-side \"right\"");
        assert_eq!(config.window_controls_side, ControlsSide::Right);

        // Unknown values fall back to the left-side default.
        let config = Config::parse("window-controls-side \"sideways\"");
        assert_eq!(config.window_controls_side, ControlsSide::Left);
    }

    #[test]
    fn test_window_controls_side_roundtrips() {
        let mut config = Config::default();
        config.window_controls_side = ControlsSide::Left;
        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.window_controls_side, ControlsSide::Left);
    }

    #[test]
    fn test_cursor_defaults_to_block_for_nav_bar_for_insert() {
        let config = Config::default();
        assert_eq!(config.cursor.insert, CursorShape::Bar);
        assert_eq!(config.cursor.normal, CursorShape::Block);
        assert_eq!(config.cursor.visual, CursorShape::Block);
    }

    #[test]
    fn test_cursor_block_roundtrips_through_kdl() {
        let mut config = Config::default();
        config.cursor.normal = CursorShape::Underline;
        config.cursor.visual = CursorShape::Bar;
        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.cursor.insert, CursorShape::Bar);
        assert_eq!(parsed.cursor.normal, CursorShape::Underline);
        assert_eq!(parsed.cursor.visual, CursorShape::Bar);
    }

    #[test]
    fn test_to_kdl_roundtrips_scalar_settings() {
        let mut config = Config::default();
        config.theme = ThemeSetting::Light;
        config.menu_style = MenuStyle::Classic;
        config.font_family = Some("Fira Code".to_string());
        config.font_size = 18.0;
        config.opacity = 0.8;
        config.status_bar.enabled = false;

        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.theme, ThemeSetting::Light);
        assert_eq!(parsed.menu_style, MenuStyle::Classic);
        assert_eq!(parsed.font_family.as_deref(), Some("Fira Code"));
        assert_eq!(parsed.font_size, 18.0);
        assert_eq!(parsed.opacity, 0.8);
        assert!(!parsed.status_bar.enabled);
        assert!(parsed.status_bar.show_mode);
    }

    #[test]
    fn test_to_kdl_roundtrips_shell() {
        let mut config = Config::default();
        config.shell = Some("/usr/bin/fish".to_string());
        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.shell.as_deref(), Some("/usr/bin/fish"));
    }

    #[test]
    fn test_to_kdl_omits_shell_when_none() {
        let kdl = Config::default().to_kdl();
        assert!(!kdl.contains("shell"), "shell should not appear when None");
    }

    #[test]
    fn test_parse_shell_linux() {
        let config = Config::parse("shell-linux \"/usr/bin/fish\"");
        assert_eq!(config.shell_linux.as_deref(), Some("/usr/bin/fish"));
        assert_eq!(config.shell_macos, None);
        assert_eq!(config.shell_windows, None);
    }

    #[test]
    fn test_parse_shell_macos() {
        let config = Config::parse("shell-macos \"/bin/zsh\"");
        assert_eq!(config.shell_macos.as_deref(), Some("/bin/zsh"));
        assert_eq!(config.shell_linux, None);
    }

    #[test]
    fn test_parse_shell_windows() {
        let config = Config::parse("shell-windows \"pwsh.exe\"");
        assert_eq!(config.shell_windows.as_deref(), Some("pwsh.exe"));
        assert_eq!(config.shell_linux, None);
    }

    #[test]
    fn test_active_shell_prefers_os_specific_over_generic() {
        let mut config = Config::default();
        config.shell = Some("/bin/sh".to_string());
        #[cfg(target_os = "windows")]
        {
            config.shell_windows = Some("pwsh.exe".to_string());
        }
        #[cfg(target_os = "macos")]
        {
            config.shell_macos = Some("/usr/local/bin/fish".to_string());
        }
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            config.shell_linux = Some("/usr/bin/fish".to_string());
        }

        #[cfg(target_os = "windows")]
        assert_eq!(config.active_shell(), Some("pwsh.exe"));
        #[cfg(target_os = "macos")]
        assert_eq!(config.active_shell(), Some("/usr/local/bin/fish"));
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        assert_eq!(config.active_shell(), Some("/usr/bin/fish"));
    }

    #[test]
    fn test_active_shell_falls_back_to_generic() {
        let mut config = Config::default();
        config.shell = Some("/bin/sh".to_string());
        assert_eq!(config.active_shell(), Some("/bin/sh"));
    }

    #[test]
    fn test_active_shell_none_when_all_unset() {
        assert_eq!(Config::default().active_shell(), None);
    }

    #[test]
    fn test_to_kdl_roundtrips_shell_linux() {
        let mut config = Config::default();
        config.shell_linux = Some("/usr/bin/fish".to_string());
        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.shell_linux.as_deref(), Some("/usr/bin/fish"));
        assert_eq!(parsed.shell_macos, None);
        assert_eq!(parsed.shell_windows, None);
    }

    #[test]
    fn test_to_kdl_roundtrips_all_shell_variants() {
        let mut config = Config::default();
        config.shell_linux = Some("/usr/bin/fish".to_string());
        config.shell_macos = Some("/opt/homebrew/bin/fish".to_string());
        config.shell_windows = Some("pwsh.exe".to_string());
        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.shell_linux.as_deref(), Some("/usr/bin/fish"));
        assert_eq!(
            parsed.shell_macos.as_deref(),
            Some("/opt/homebrew/bin/fish")
        );
        assert_eq!(parsed.shell_windows.as_deref(), Some("pwsh.exe"));
        assert_eq!(parsed.shell, None);
    }

    #[test]
    fn test_theme_setting_from_and_to_value() {
        assert_eq!(ThemeSetting::from_value("auto"), ThemeSetting::Auto);
        assert_eq!(ThemeSetting::from_value("light"), ThemeSetting::Light);
        assert_eq!(ThemeSetting::from_value("dark"), ThemeSetting::Dark);
        assert_eq!(
            ThemeSetting::from_value("dracula"),
            ThemeSetting::Named("dracula".to_string())
        );
        assert_eq!(ThemeSetting::Named("nord".to_string()).as_value(), "nord");
        assert_eq!(ThemeSetting::Auto.as_value(), "auto");
    }

    #[test]
    fn test_parse_and_roundtrip_named_theme() {
        let config = Config::parse("theme \"dracula\"");
        assert_eq!(config.theme, ThemeSetting::Named("dracula".to_string()));
        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.theme, ThemeSetting::Named("dracula".to_string()));
    }

    #[test]
    fn test_translucent_colors_are_laid_over_the_background_and_saved_as_written() {
        let mut config = Config::default();
        config.colors.background = ThemeRgb::parse_hex("#00000080");
        config
            .colors
            .set_scalar("selection-bg", ThemeRgb::parse_hex("#ffffff80"));
        config
            .colors
            .git
            .set("file-band", ThemeRgb::parse_hex("#ff000000"));
        config
            .colors
            .syntax
            .set("comment", ThemeRgb::parse_hex("#11223380"));

        let mut theme = Theme::dark();
        config.colors.apply(&mut theme);
        assert_eq!(theme.background_alpha, 0x80, "the window shows through");
        assert_eq!(theme.background, ThemeRgb::new(0, 0, 0));
        assert_eq!(theme.selection_bg, ThemeRgb::new(128, 128, 128));
        assert_eq!(
            theme.git.file_band,
            ThemeRgb::parse_hex("#ff000000"),
            "kept as written"
        );
        assert!(theme.selection_bg.is_opaque());
        assert_eq!(
            theme.git.hunk_band,
            Theme::dark().git.hunk_band,
            "unset keeps the preset's"
        );

        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(
            parsed.colors.scalar("selection-bg"),
            ThemeRgb::parse_hex("#ffffff80")
        );
        assert_eq!(
            parsed.colors.git.get("file-band"),
            ThemeRgb::parse_hex("#ff000000")
        );
        assert_eq!(
            parsed.colors.syntax.get("comment"),
            ThemeRgb::parse_hex("#11223380")
        );
        assert_eq!(
            theme.syntax.get("comment"),
            ThemeRgb::parse_hex("#11223380"),
            "kept as written"
        );
        assert_eq!(
            theme.syntax.get("string"),
            Theme::dark().syntax.get("string"),
            "unset keeps the preset's"
        );
    }

    #[test]
    fn test_to_kdl_roundtrips_color_overrides() {
        let mut config = Config::default();
        config.colors.background = ThemeRgb::parse_hex("#1a1a2e");
        config
            .colors
            .set_scalar("cursor-bg", ThemeRgb::parse_hex("#52ad70"));
        config
            .colors
            .set_scalar("window-border", ThemeRgb::parse_hex("#123456"));
        config
            .colors
            .set_scalar("active-indicator", ThemeRgb::parse_hex("#e0457b"));
        config.colors.ansi[0] = ThemeRgb::parse_hex("#000000");
        config.colors.indexed = vec![(136, ThemeRgb::parse_hex("#af8700").unwrap())];

        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.colors.background, ThemeRgb::parse_hex("#1a1a2e"));
        assert_eq!(
            parsed.colors.scalar("cursor-bg"),
            ThemeRgb::parse_hex("#52ad70")
        );
        assert_eq!(
            parsed.colors.scalar("window-border"),
            ThemeRgb::parse_hex("#123456")
        );
        assert_eq!(
            parsed.colors.scalar("active-indicator"),
            ThemeRgb::parse_hex("#e0457b")
        );
        assert_eq!(parsed.colors.ansi[0], ThemeRgb::parse_hex("#000000"));
        assert!(parsed.colors.ansi[1..].iter().all(Option::is_none));
        assert_eq!(
            parsed.colors.indexed,
            vec![(136, ThemeRgb::parse_hex("#af8700").unwrap())]
        );
    }

    #[test]
    fn test_to_kdl_without_overrides_writes_no_colors_block() {
        let config = Config::default();
        let kdl = config.to_kdl();
        assert!(!kdl.contains("colors {"), "{kdl}");
        // A pristine config still round-trips to its defaults.
        let parsed = Config::parse(&kdl);
        assert_eq!(parsed.theme, ThemeSetting::Dark);
        assert_eq!(parsed.font_size, 15.0);
    }

    #[test]
    fn test_status_bar_show_mode_defaults_on_and_parses_off() {
        let default = Config::default().status_bar;
        assert!(default.enabled && default.show_mode);

        let config = Config::parse(
            r#"
status-bar {
    show #false
}
"#,
        );
        assert!(!config.status_bar.enabled);
        assert!(config.status_bar.show_mode);

        let config = Config::parse(
            r#"
status-bar {
    show #true
    show-mode #false
}
"#,
        );
        assert!(config.status_bar.enabled);
        assert!(!config.status_bar.show_mode);
    }

    #[test]
    fn test_status_bar_priority_is_read_and_written_back() {
        let (config, warning) = Config::parse_checked(
            r#"
status-bar {
    segments "tool" "claude"
    priority "claude" "tool"
}
"#,
        );
        assert_eq!(warning, None);
        assert_eq!(
            config.status_bar.priority,
            [StatusSegmentKind::Claude, StatusSegmentKind::Editor]
        );
        let again = Config::parse_checked(&config.to_kdl()).0;
        assert_eq!(again.status_bar.priority, config.status_bar.priority);
        assert!(Config::parse_checked("").0.status_bar.priority.is_empty());
    }

    #[test]
    fn test_status_bar_segments_are_picked_and_ordered_by_name() {
        let (config, warning) = Config::parse_checked(
            r#"
status-bar {
    segments "tool" "zai" "nope" "claude"
}
"#,
        );
        assert_eq!(
            config.status_bar.segments,
            [
                StatusSegmentKind::Editor,
                StatusSegmentKind::Zai,
                StatusSegmentKind::Claude,
            ]
        );
        assert!(
            warning.is_some_and(|w| w.contains("nope")),
            "an unknown name is said, not silently dropped"
        );

        let one = Config::parse("status-bar {\n    segments \"claude\"\n}\n");
        assert_eq!(one.status_bar.segments, [StatusSegmentKind::Claude]);

        let none = Config::parse("status-bar {\n    segments\n}\n");
        assert!(
            none.status_bar.segments.is_empty(),
            "an empty list hides all"
        );

        let unset = Config::parse("status-bar {\n    show #true\n}\n");
        assert_eq!(unset.status_bar.segments, StatusSegmentKind::ALL);
    }

    #[test]
    fn test_status_bar_segments_round_trip_through_settings_kdl() {
        for segments in [
            vec![StatusSegmentKind::Zai, StatusSegmentKind::Editor],
            Vec::new(),
        ] {
            let mut config = Config::default();
            config.status_bar.segments = segments.clone();
            assert_eq!(
                Config::parse(&config.to_kdl()).status_bar.segments,
                segments
            );
        }
    }

    #[test]
    fn test_cursor_blink_parses_and_round_trips() {
        let c = Config::default();
        assert!(c.cursor.blink, "blink defaults to true");

        let no_blink = Config::parse("cursor {\n    blink #false\n}");
        assert!(!no_blink.cursor.blink);

        // Round-trip via to_kdl / parse.
        let kdl = no_blink.to_kdl();
        let restored = Config::parse(&kdl);
        assert!(!restored.cursor.blink);
    }

    #[test]
    fn test_clipboard_read_parses_and_round_trips() {
        let c = Config::default();
        assert!(
            !c.clipboard_read,
            "clipboard_read defaults to false (refuse)"
        );

        let enabled = Config::parse(r"clipboard-read #true");
        assert!(enabled.clipboard_read);

        let kdl = enabled.to_kdl();
        let restored = Config::parse(&kdl);
        assert!(restored.clipboard_read);
    }

    #[test]
    fn test_ligatures_parses_and_round_trips() {
        let c = Config::default();
        assert!(!c.ligatures, "ligatures defaults to false");

        let lig = Config::parse(r#"ligatures #true"#);
        assert!(lig.ligatures);

        let kdl = lig.to_kdl();
        let restored = Config::parse(&kdl);
        assert!(restored.ligatures);
    }

    #[test]
    fn test_url_underline_parses_and_round_trips() {
        let c = Config::default();
        assert!(!c.url_underline, "url_underline defaults to false");

        let underline = Config::parse(r#"url-underline #true"#);
        assert!(underline.url_underline);

        let kdl = underline.to_kdl();
        let restored = Config::parse(&kdl);
        assert!(restored.url_underline);
    }

    #[test]
    fn test_wrap_indent_parses_and_round_trips() {
        let c = Config::default();
        assert!(c.wrap_indent, "wrap_indent defaults to true");

        let disabled = Config::parse(r#"wrap-indent #false"#);
        assert!(!disabled.wrap_indent);

        let kdl = disabled.to_kdl();
        let restored = Config::parse(&kdl);
        assert!(!restored.wrap_indent);
    }

    #[test]
    fn test_wrap_words_parses_and_round_trips() {
        let c = Config::default();
        assert!(c.wrap_words, "wrap_words defaults to true");

        let disabled = Config::parse(r#"wrap-words #false"#);
        assert!(!disabled.wrap_words);

        let kdl = disabled.to_kdl();
        let restored = Config::parse(&kdl);
        assert!(!restored.wrap_words);
    }

    #[test]
    fn test_rainbow_parens_parses_and_round_trips() {
        let c = Config::default();
        assert!(!c.rainbow_parens, "rainbow_parens defaults to false");

        let enabled = Config::parse(r#"rainbow-parens #true"#);
        assert!(enabled.rainbow_parens);

        let kdl = enabled.to_kdl();
        let restored = Config::parse(&kdl);
        assert!(restored.rainbow_parens);
    }

    #[test]
    fn test_sentence_highlight_parses_and_round_trips() {
        let c = Config::default();
        assert!(
            !c.sentence_highlight,
            "sentence_highlight defaults to false"
        );

        let enabled = Config::parse(r#"sentence-highlight #true"#);
        assert!(enabled.sentence_highlight);

        let kdl = enabled.to_kdl();
        let restored = Config::parse(&kdl);
        assert!(restored.sentence_highlight);
    }

    #[test]
    fn test_a_config_naming_the_retired_icons_key_still_loads() {
        // Every settings.kdl written before the icon style was retired names
        // it, and the schema rejects unknown keys: without the key still being
        // accepted, the whole file would be thrown away over one dead line.
        let parsed = Config::parse("icons \"font\"\nfont-size 21");
        assert_eq!(parsed.font_size, 21.0);
        assert!(
            !parsed.to_kdl().contains("icons"),
            "and it is not written back"
        );
    }

    #[test]
    fn test_prompt_edit_bindings_parse_and_round_trip() {
        assert_eq!(
            Config::default().prompt_edit_bindings,
            PromptEditBindings::Emacs
        );

        let off = Config::parse(r#"prompt-edit-bindings "none""#);
        assert_eq!(off.prompt_edit_bindings, PromptEditBindings::None);
        assert_eq!(
            Config::parse(&off.to_kdl()).prompt_edit_bindings,
            PromptEditBindings::None
        );

        // Spellings a user is likely to reach for.
        for spelling in ["none", "off", "vi"] {
            let text = format!(r#"prompt-edit-bindings "{spelling}""#);
            assert_eq!(
                Config::parse(&text).prompt_edit_bindings,
                PromptEditBindings::None,
                "{spelling} should disable prompt editing"
            );
        }
    }

    #[test]
    fn test_unknown_prompt_edit_bindings_keeps_the_default() {
        // A typo must not silently disable a working feature.
        let c = Config::parse(r#"prompt-edit-bindings "emcas""#);
        assert_eq!(c.prompt_edit_bindings, PromptEditBindings::Emacs);
    }

    #[test]
    fn test_a_config_naming_the_retired_block_settings_still_loads() {
        // Settings files written while blocks ran in a web engine name its
        // trust ceiling, its network switch, and the block-focus cursor; the
        // schema rejects unknown keys, so each must still be accepted.
        let parsed = Config::parse(
            r#"cursor {
                block-focus "bar"
                insert "block"
            }
            security {
                block-max-trust "trusted"
                block-remote-assets #true
            }
            font-size 21"#,
        );
        assert_eq!(parsed.font_size, 21.0, "the rest of the file still applies");
        assert_eq!(parsed.cursor.insert, CursorShape::Block);
        let written = parsed.to_kdl();
        assert!(
            !written.contains("security"),
            "and they are not written back"
        );
        assert!(!written.contains("block-focus"));
    }

    #[test]
    fn test_parse_checked_reports_a_rejected_file() {
        let (config, error) = Config::parse_checked("this is not kdl {{{");
        assert_eq!(config.font_size, Config::default().font_size);
        let message = error.expect("a rejected file must report why");
        assert!(
            message.contains("settings.kdl"),
            "diagnostic should name the file: {message}"
        );
    }

    #[test]
    fn test_a_syntax_color_no_scope_goes_by_is_reported_and_the_rest_kept() {
        let (config, warning) = Config::parse_checked(
            r##"colors { syntax { keyword "#010203"; entity.name.function.rust "#040506" } }"##,
        );
        let message = warning.expect("an unknown scope is reported");
        assert!(message.contains("entity.name.function.rust"), "{message}");
        assert_eq!(
            config.colors.syntax.get("keyword"),
            ThemeRgb::parse_hex("#010203")
        );
    }

    #[test]
    fn test_parse_checked_reports_nothing_for_a_good_file() {
        let (_, error) = Config::parse_checked("font-size 17");
        assert!(error.is_none());
    }

    /// The kebab-case names of the settings the schema accepts, read out of the
    /// schema's own source so the list cannot fall behind it. `keybindings` is
    /// left out: it is legal here but documented to live in its own file.
    fn schema_keys(schema: &str) -> Vec<String> {
        let start = schema
            .find("struct KdlConfig {")
            .expect("the schema declares KdlConfig");
        schema[start..]
            .lines()
            .skip(1)
            .take_while(|line| !line.starts_with('}'))
            .filter_map(|line| line.trim().strip_prefix("pub(crate) "))
            .filter_map(|field| field.split(':').next())
            .filter(|name| *name != "keybindings")
            .map(|name| name.replace('_', "-"))
            .collect()
    }

    /// Whether `text` writes `key` as a settings key, set or commented out,
    /// rather than merely mentioning it in a sentence.
    fn sets_key(text: &str, key: &str) -> bool {
        text.lines().any(|line| {
            let line = line.trim_start().trim_start_matches('/').trim_start();
            line.strip_prefix(key)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '{']))
        })
    }

    #[test]
    fn test_every_setting_appears_in_the_shipped_sample() {
        // The sample's header promises every option at its default value, and
        // the installers ship it as the reference users copy from: a setting
        // added to the schema and not to the file is one nobody learns exists.
        // The keys are read from the schema source rather than listed here,
        // because a hand-kept list is the thing that fell behind in the first
        // place.
        let schema = include_str!("schema.rs");
        let sample = include_str!("../../samples/settings.kdl");
        let missing: Vec<String> = schema_keys(schema)
            .into_iter()
            .filter(|key| !sets_key(sample, key))
            .collect();
        assert!(
            missing.is_empty(),
            "missing from samples/settings.kdl: {missing:?}"
        );
    }

    #[test]
    fn test_every_setting_survives_a_save() {
        // Saving rewrites the whole file from the live config, so a key `to_kdl`
        // skips is one the user loses the moment they change anything else on
        // the settings page. Optional settings are given values here, since a
        // config that never set them has nothing to write.
        let config = Config {
            font_family: Some("Fira Code".to_string()),
            font_weight: Some("regular".to_string()),
            font_weight_bold: Some("bold".to_string()),
            scrollback_lines: Some(5_000),
            shell: Some("/bin/zsh".to_string()),
            shell_linux: Some("/bin/zsh".to_string()),
            shell_macos: Some("/bin/zsh".to_string()),
            shell_windows: Some("pwsh.exe".to_string()),
            ..Config::default()
        };
        let written = config.to_kdl();
        let schema = include_str!("schema.rs");
        let dropped: Vec<String> = schema_keys(schema)
            .into_iter()
            // A color table is only written when something overrides the theme.
            .filter(|key| key != "colors")
            .filter(|key| !sets_key(&written, key))
            .collect();
        assert!(dropped.is_empty(), "not written back on save: {dropped:?}");
    }

    #[test]
    fn test_the_settings_a_save_used_to_drop_come_back() {
        // These three were written by no one: a save rebuilt the file without
        // them, so each reverted to its default behind the user's back. Values
        // opposite to the defaults, so a dropped key cannot read as a pass.
        let mut config = Config::default();
        config.cursor.hide_in_inactive = !config.cursor.hide_in_inactive;
        config.dim_inactive = !config.dim_inactive;
        config.restore_session = !config.restore_session;

        let reloaded = Config::parse(&config.to_kdl());
        assert_eq!(reloaded.dim_inactive, config.dim_inactive);
        assert_eq!(reloaded.restore_session, config.restore_session);
        assert_eq!(
            reloaded.cursor.hide_in_inactive,
            config.cursor.hide_in_inactive
        );
    }

    #[test]
    fn test_shipped_sample_settings_parse_cleanly() {
        // `samples/settings.kdl` is installed as documentation by the .deb and
        // is the file users copy; a stale key here would hand them a config
        // that silently reverts to defaults.
        let sample = include_str!("../../samples/settings.kdl");
        let (_, error) = Config::parse_checked(sample);
        assert!(
            error.is_none(),
            "sample settings.kdl failed to parse: {error:?}"
        );
    }

    #[test]
    fn test_a_font_size_the_renderer_cannot_draw_is_held_to_one_it_can() {
        // `font-size 0` used to crash every launch: startup took it as is.
        assert_eq!(Config::parse("font-size 0").font_size, 6.0);
        assert_eq!(Config::parse("font-size -3").font_size, 6.0);
        assert_eq!(Config::parse("font-size 900").font_size, 72.0);
    }

    #[test]
    fn test_a_keybindings_file_that_does_not_parse_says_so_without_rejecting_the_settings() {
        let (config, settings_error) =
            Config::parse_with_keys_checked("font-size 17", "window {{{");
        assert_eq!(settings_error, None, "the settings still apply");
        assert_eq!(config.font_size, 17.0);
        assert!(
            config
                .keybindings_error
                .as_deref()
                .is_some_and(|e| e.starts_with("keybindings.kdl ignored")),
            "{:?}",
            config.keybindings_error
        );
    }

    #[test]
    fn test_a_keybindings_file_that_does_not_parse_names_the_line_at_fault() {
        let keys = "window {\n    \"C+h\" \"focus_left\"\n}\neditor {{\n";
        let (config, _) = Config::parse_with_keys_checked("", keys);
        let error = config.keybindings_error.expect("the file is rejected");
        assert!(error.contains("line 4"), "{error}");
    }

    #[test]
    fn test_the_active_indicator_color_comes_from_the_colors_block() {
        let config = Config::parse("colors {\n    active-indicator \"#e0457b\"\n}");
        let mut theme = Theme::dark();
        config.colors.apply(&mut theme);
        assert_eq!(
            theme.active_indicator,
            ThemeRgb::parse_hex("#e0457b").unwrap()
        );
        // Untouched, it is the preset's.
        let mut preset = Theme::dark();
        Config::parse("").colors.apply(&mut preset);
        assert_eq!(preset.active_indicator, Theme::dark().active_indicator);
    }
}
