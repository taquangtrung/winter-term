//! KDL configuration parser: theme, fonts, keybindings, and appearance.
//!
//! Configuration lives in `~/.config/winter-term/` (or `$XDG_CONFIG_HOME/winter-term`;
//! `%APPDATA%\winter-term` on Windows) split across two files:
//! - `settings.kdl` — appearance and behavior (theme, fonts, colors, menu style,
//!   status bar).
//! - `keybindings.kdl` — keybindings, as top-level mode blocks.
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
//!     block-focus "bar"
//! }
//!
//! colors {
//!     background "#2a2f31"
//!     foreground "#d8d8d8"
//!     cursor-bg "#52ad70"
//!     selection-bg "#fffacd"
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
//! // prev_tab, next_tab, new_tab, close_tab, copy_selection,
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

pub(crate) mod watch;

use std::collections::HashMap;
use std::path::PathBuf;

use winter_core::winter_proto::TrustTier;
use winter_render::{ControlsSide, CursorShape, MenuStyle, Theme, ThemeRgb};

/// The window title template used when `window-title-template` is unset (or set
/// to empty). `{{ title }}` is the active tab's resolved title, so the default
/// keeps the long-standing `"Winter - <title>"` look.
pub(crate) const DEFAULT_WINDOW_TITLE_TEMPLATE: &str = "Winter - {{ title }}";

// ========================================================================
// Data Structures
// ========================================================================

/// Which glyphs the status bar draws for each indicator.
#[derive(Clone, Debug)]
pub struct StatusBarIconsConfig {
    pub normal: String,
    pub insert: String,
    pub block: String,
}

impl Default for StatusBarIconsConfig {
    fn default() -> Self {
        Self {
            normal: "\u{e795}".to_string(),  // 
            insert: "\u{f03eb}".to_string(), // 󰏫
            block: "\u{f0485}".to_string(),  // 󰒅
        }
    }
}

/// Status bar visibility and content. `enabled` toggles the whole bar (which
/// also frees its reserved row); `show_mode` toggles the mode indicator.
#[derive(Clone, Debug)]
pub struct StatusBarConfig {
    pub enabled: bool,
    pub icons: StatusBarIconsConfig,
    pub show_mode: bool,
}

impl Default for StatusBarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            icons: StatusBarIconsConfig::default(),
            show_mode: true,
        }
    }
}

/// Everything the configuration files can set.
#[derive(Clone, Debug)]
pub struct Config {
    /// Let programs read the clipboard through OSC 52 (`ESC ] 52 ; c ; ?`).
    /// Opt-in because the query is silent and unconditional on the querying
    /// side — any program in the pane (including one behind ssh) could pull
    /// the clipboard's contents. Responses are capped (64 KiB of text).
    /// Default: `false`.
    pub clipboard_read: bool,
    pub colors: ColorOverrides,
    pub cursor: CursorConfig,
    /// Dim inactive panes by blending their colors toward the background. Default: `false`.
    pub dim_inactive: bool,
    pub font_family: Option<String>,
    pub font_size: f32,
    pub font_weight: Option<String>,
    pub font_weight_bold: Option<String>,
    pub keybindings: HashMap<String, HashMap<String, String>>,
    /// Enable OpenType ligatures in the font renderer. Defaults to `false`:
    /// like most terminals, winter keeps each cell's glyph at its nominal
    /// position so contextual shaping never breaks the monospace grid (e.g.
    /// raising the middle asterisk of `***`). Set `ligatures #true` to opt in.
    pub ligatures: bool,
    /// Tabbar menu presentation: a modern hamburger dropdown or a classic menubar.
    pub menu_style: MenuStyle,
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
    /// Policy governing what rich blocks arriving over TBP are allowed to do.
    pub security: SecurityConfig,
    /// Alternating background bands per sentence. Default: `false`.
    pub sentence_highlight: bool,
    pub shell: Option<String>,
    pub shell_windows: Option<String>,
    pub shell_linux: Option<String>,
    pub shell_macos: Option<String>,
    pub status_bar: StatusBarConfig,
    pub theme: ThemeSetting,
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
    /// first non-blank column. Default: `true`.
    pub wrap_indent: bool,
}

/// Which line-editor bindings Winter assumes the shell uses when it realizes a
/// Vim prompt-line operator.
///
/// Normal-mode operators on the prompt line are sent to the shell as the
/// equivalent line-editor keystrokes, so Winter has to know which set the shell
/// answers to. It cannot detect this: the shell never reports its editing mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PromptEditBindings {
    /// Readline/ZLE emacs bindings (`Ctrl-A`, `Ctrl-K`, `Ctrl-U`, `Ctrl-W`,
    /// `Alt-d`), which is every common shell's default.
    #[default]
    Emacs,
    /// Do not translate. For a shell already in vi mode (`bindkey -v`,
    /// `set editing-mode vi`, fish's vi bindings), where those chords mean
    /// something else and the shell provides its own Vim editing anyway.
    None,
}

/// Policy for rich TBP blocks, parsed from the `security` block.
///
/// Every value a block carries arrives over a PTY, and nothing on a PTY is
/// authenticated: a `cat` of a downloaded file, output piped from `curl`, or a
/// program on the far side of `ssh` can all spell any escape a first-party tool
/// can. So the defaults here grant nothing, and raising them is a deliberate
/// decision the user makes about the tools they run.
#[derive(Clone, Copy, Debug)]
pub struct SecurityConfig {
    /// Ceiling applied to the trust tier a block *requests* on the wire, via
    /// [`TrustTier::clamp_to`]. Default: [`TrustTier::Restricted`], which
    /// renders blocks under a CSP with scripting off.
    ///
    /// Raising this to `trusted` grants scripting to *any* byte stream that
    /// reaches a pane, not just to tools you trust; there is no way for the
    /// terminal to tell them apart.
    pub block_max_trust: TrustTier,
    /// Let block content load subresources from the network (the Vega/Vega-Lite
    /// renderer's CDN bundles). Default: `false`, so rendering a block never
    /// makes a network request the user did not initiate.
    pub block_remote_assets: bool,
}

/// Per-mode cursor shapes, parsed from the `cursor` block. Each mode renders
/// its cursor with the configured shape; defaults follow the common terminal
/// convention (a bar for insert-like modes, a block for navigation).
#[derive(Clone, Copy, Debug)]
pub struct CursorConfig {
    /// Whether the cursor blinks. Applies only to the focused pane.
    pub blink: bool,
    pub block_focus: CursorShape,
    /// Hide the cursor entirely in inactive (non-focused) panes. Default: `true`.
    pub hide_in_inactive: bool,
    pub insert: CursorShape,
    pub normal: CursorShape,
    pub visual: CursorShape,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            blink: true,
            block_focus: CursorShape::Bar,
            hide_in_inactive: true,
            insert: CursorShape::Bar,
            normal: CursorShape::Block,
            visual: CursorShape::Block,
        }
    }
}

/// Per-color overrides parsed from the KDL `colors` block. Applied on top of the
/// active preset ([`Theme::dark`]/[`Theme::light`]); unset colors keep the preset.
#[derive(Clone, Debug, Default)]
pub struct ColorOverrides {
    /// The 8 standard ANSI colors, indexed in [`ANSI_COLOR_NAMES`] order. A
    /// `None` slot leaves that color at the active preset's value.
    pub ansi: [Option<ThemeRgb>; 8],
    pub background: Option<ThemeRgb>,
    pub bell: Option<ThemeRgb>,
    /// The 8 bright ANSI colors, same slot order as [`Self::ansi`].
    pub brights: [Option<ThemeRgb>; 8],
    pub cursor_bg: Option<ThemeRgb>,
    pub cursor_fg: Option<ThemeRgb>,
    pub divider: Option<ThemeRgb>,
    pub foreground: Option<ThemeRgb>,
    pub scrollbar: Option<ThemeRgb>,
    pub status_bar_border: Option<ThemeRgb>,
    pub indexed: Vec<(u8, ThemeRgb)>,
    pub selection_bg: Option<ThemeRgb>,
    pub selection_fg: Option<ThemeRgb>,
    pub window_border: Option<ThemeRgb>,
}

impl ColorOverrides {
    /// Overwrite the matching fields of `theme` with any colors set here.
    pub fn apply(&self, theme: &mut Theme) {
        if let Some(c) = self.background {
            theme.background = c;
        }
        if let Some(c) = self.foreground {
            theme.foreground = c;
        }
        if let Some(c) = self.cursor_bg {
            theme.cursor_bg = c;
        }
        if let Some(c) = self.cursor_fg {
            theme.cursor_fg = c;
        }
        if let Some(c) = self.selection_bg {
            theme.selection_bg = c;
        }
        if let Some(c) = self.selection_fg {
            theme.selection_fg = c;
        }
        if let Some(c) = self.divider {
            theme.divider = c;
        }
        if let Some(c) = self.scrollbar {
            theme.scrollbar = c;
        }
        if let Some(c) = self.status_bar_border {
            theme.status_bar_border = c;
        }
        if let Some(c) = self.window_border {
            theme.window_border = c;
        }
        if let Some(c) = self.bell {
            theme.bell = Some(c);
        }
        for (i, c) in self.ansi.iter().enumerate() {
            if let Some(c) = c {
                theme.ansi[i] = *c;
            }
        }
        for (i, c) in self.brights.iter().enumerate() {
            if let Some(c) = c {
                theme.ansi[8 + i] = *c;
            }
        }
        for (idx, c) in &self.indexed {
            if let Some(slot) = theme.indexed.iter_mut().find(|(i, _)| i == idx) {
                slot.1 = *c;
            } else {
                theme.indexed.push((*idx, *c));
            }
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
    Auto,
    #[default]
    Dark,
    Light,
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

#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct KdlConfig {
    colors: Option<KdlColors>,
    cursor: Option<KdlCursor>,
    dim_inactive: Option<bool>,
    font: Option<String>,
    font_size: Option<f32>,
    font_weight: Option<String>,
    font_weight_bold: Option<String>,
    keybindings: Option<HashMap<String, HashMap<String, String>>>,
    menu_style: Option<String>,
    scrollback_lines: Option<u64>,
    security: Option<KdlSecurity>,
    shell: Option<String>,
    shell_windows: Option<String>,
    shell_linux: Option<String>,
    shell_macos: Option<String>,
    title_bar_style: Option<String>,
    ligatures: Option<bool>,
    clipboard_read: Option<bool>,
    palette_match_underline: Option<bool>,
    pane_border_width: Option<f32>,
    paste_on_right_click: Option<bool>,
    prompt_edit_bindings: Option<String>,
    rainbow_parens: Option<bool>,
    restore_session: Option<bool>,
    sentence_highlight: Option<bool>,
    url_underline: Option<bool>,
    window_controls_side: Option<String>,
    window_title_template: Option<String>,
    wrap_indent: Option<bool>,
    opacity: Option<f32>,
    theme: Option<String>,
    status_bar: Option<KdlStatusBar>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct KdlCursor {
    blink: Option<bool>,
    block_focus: Option<String>,
    hide_in_inactive: Option<bool>,
    insert: Option<String>,
    normal: Option<String>,
    visual: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct KdlSecurity {
    block_max_trust: Option<String>,
    block_remote_assets: Option<bool>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct KdlStatusBar {
    normal_icon: Option<String>,
    insert_icon: Option<String>,
    block_icon: Option<String>,
    show: Option<bool>,
    show_mode: Option<bool>,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct KdlColors {
    background: Option<String>,
    foreground: Option<String>,
    cursor_bg: Option<String>,
    cursor_fg: Option<String>,
    scrollbar: Option<String>,
    selection_bg: Option<String>,
    selection_fg: Option<String>,
    split: Option<String>,
    status_bar_border: Option<String>,
    visual_bell: Option<String>,
    window_border: Option<String>,
    // Nested blocks: ansi { red "#c22727" }, brights { red "#d43f30" }
    ansi: Option<HashMap<String, String>>,
    brights: Option<HashMap<String, String>>,
    // Nested block: indexed { "136" "#af8700" }
    indexed: Option<HashMap<String, String>>,
}

/// A `themes/<name>.kdl` file: an optional `base` preset (`dark`/`light`) plus a
/// `colors` block layered over it, reusing the same color schema as the main
/// config.
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct KdlThemeFile {
    base: Option<String>,
    colors: Option<KdlColors>,
}

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

    /// Read a config file, falling back to defaults when it is absent or unreadable.
    pub fn load_from(path: &PathBuf) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(_) => return Self::default(),
        };
        Self::parse(&text)
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
        let parsed = parse_keys(keys);
        if !parsed.is_empty() {
            config.keybindings = parsed;
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

        let status_bar = kdl
            .status_bar
            .map(|sb| {
                let defaults = StatusBarIconsConfig::default();
                StatusBarConfig {
                    enabled: sb.show.unwrap_or(true),
                    icons: StatusBarIconsConfig {
                        normal: sb.normal_icon.unwrap_or(defaults.normal),
                        insert: sb.insert_icon.unwrap_or(defaults.insert),
                        block: sb.block_icon.unwrap_or(defaults.block),
                    },
                    show_mode: sb.show_mode.unwrap_or(true),
                }
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
            colors: kdl.colors.map(color_overrides_from_kdl).unwrap_or_default(),
            cursor: kdl.cursor.map(cursor_config_from_kdl).unwrap_or_default(),
            dim_inactive: kdl.dim_inactive.unwrap_or(false),
            font_family: kdl.font.filter(|s| !s.trim().is_empty()),
            font_size: kdl.font_size.unwrap_or(15.0),
            font_weight: kdl.font_weight.filter(|s| !s.trim().is_empty()),
            font_weight_bold: kdl.font_weight_bold.filter(|s| !s.trim().is_empty()),
            keybindings,
            menu_style,
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
            security: kdl
                .security
                .map(security_config_from_kdl)
                .unwrap_or_default(),
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
        };
        (config, None)
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
        out.push_str(&format!(
            "title-bar-style {}\n",
            kdl_string(self.title_bar_style.as_value())
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
        out.push_str(&self.security_kdl());
        out.push_str(&self.status_bar_kdl());
        if let Some(colors) = self.colors_kdl() {
            out.push_str(&colors);
        }
        out
    }

    /// Write [`Self::to_kdl`] to `settings.kdl`, creating the config directory if
    /// needed. Keybindings are left untouched.
    pub fn save(&self) -> std::io::Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("settings.kdl"), self.to_kdl())
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
            "    block-focus {}\n",
            kdl_string(c.block_focus.as_value())
        ));
        out.push_str("}\n");
        out
    }

    /// The `security` block, always emitted so the active policy is visible in
    /// the file rather than left implicit in the defaults.
    fn security_kdl(&self) -> String {
        let sec = &self.security;
        let mut out = String::from("security {\n");
        out.push_str(&format!(
            "    block-max-trust {}\n",
            kdl_string(sec.block_max_trust.as_str())
        ));
        out.push_str(&format!(
            "    block-remote-assets {}\n",
            kdl_bool(sec.block_remote_assets)
        ));
        out.push_str("}\n");
        out
    }

    /// The `status-bar` block, always emitted so every flag and icon round-trips.
    fn status_bar_kdl(&self) -> String {
        let sb = &self.status_bar;
        let mut out = String::from("status-bar {\n");
        out.push_str(&format!("    show {}\n", kdl_bool(sb.enabled)));
        out.push_str(&format!("    show-mode {}\n", kdl_bool(sb.show_mode)));
        out.push_str(&format!(
            "    normal-icon {}\n",
            kdl_string(&sb.icons.normal)
        ));
        out.push_str(&format!(
            "    insert-icon {}\n",
            kdl_string(&sb.icons.insert)
        ));
        out.push_str(&format!("    block-icon {}\n", kdl_string(&sb.icons.block)));
        out.push_str("}\n");
        out
    }

    /// The `colors` block, or `None` when no color override is set (so an
    /// untouched config keeps the preset and writes no block).
    fn colors_kdl(&self) -> Option<String> {
        let c = &self.colors;
        let scalars: [(&str, Option<ThemeRgb>); 11] = [
            ("background", c.background),
            ("foreground", c.foreground),
            ("cursor-bg", c.cursor_bg),
            ("cursor-fg", c.cursor_fg),
            ("scrollbar", c.scrollbar),
            ("selection-bg", c.selection_bg),
            ("selection-fg", c.selection_fg),
            ("split", c.divider),
            ("status-bar-border", c.status_bar_border),
            ("visual-bell", c.bell),
            ("window-border", c.window_border),
        ];
        let any = scalars.iter().any(|(_, v)| v.is_some())
            || c.ansi.iter().any(Option::is_some)
            || c.brights.iter().any(Option::is_some)
            || !c.indexed.is_empty();
        if !any {
            return None;
        }

        let mut out = String::from("colors {\n");
        for (name, value) in scalars {
            if let Some(rgb) = value {
                out.push_str(&format!("    {name} {}\n", kdl_string(&rgb.to_hex())));
            }
        }
        if let Some(line) = color_named_block_kdl("ansi", &c.ansi) {
            out.push_str(&line);
        }
        if let Some(line) = color_named_block_kdl("brights", &c.brights) {
            out.push_str(&line);
        }
        if !c.indexed.is_empty() {
            out.push_str("    indexed {\n");
            for (index, rgb) in &c.indexed {
                out.push_str(&format!(
                    "        {} {}\n",
                    kdl_string(&index.to_string()),
                    kdl_string(&rgb.to_hex())
                ));
            }
            out.push_str("    }\n");
        }
        out.push_str("}\n");
        Some(out)
    }
}

impl PromptEditBindings {
    /// Parse the `prompt-edit-bindings` value. An unrecognized word keeps the
    /// default rather than silently disabling prompt editing.
    fn from_value(value: &str) -> Self {
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

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            block_max_trust: TrustTier::Restricted,
            block_remote_assets: false,
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
            font_size: 15.0,
            font_weight: None,
            font_weight_bold: None,
            keybindings: HashMap::new(),
            ligatures: false,
            clipboard_read: false,
            menu_style: MenuStyle::default(),
            opacity: 1.0,
            palette_match_underline: false,
            pane_border_width: 1.0,
            paste_on_right_click: false,
            prompt_edit_bindings: PromptEditBindings::default(),
            rainbow_parens: false,
            restore_session: false,
            scrollback_lines: None,
            security: SecurityConfig::default(),
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
        }
    }
}

// ========================================================================
// Window title template
// ========================================================================

/// Values the `window-title-template` placeholders expand to, describing the
/// app running in the active pane. Fields default to empty when the running
/// program provides none (e.g. `app_name` while sitting at a shell prompt).
pub struct WindowTitleVars {
    /// The active tab's resolved title — the OSC 0/2 title set by the running
    /// app, else its process name and cwd, else `Terminal N`. Same string the
    /// tab strip shows.
    pub title: String,
    /// Name of the foreground process in the active pane (e.g. `butterfly`).
    pub app_name: String,
    /// The OSC 0/2 title exactly as the running app set it (cleaned of a
    /// `user@host:` prefix).
    pub pane_title: String,
    /// The active pane's working directory, abbreviated like the tab strip
    /// shows it (e.g. `~/W/a/winter-term`).
    pub cwd: String,
}

/// Expand the `{{ name }}` placeholders of a window-title template
/// (`window-title-template`). Surrounding whitespace inside the braces is
/// allowed (`{{title}}`, `{{ title }}`); an unknown name is left as literal
/// text so a typo stays visible instead of silently vanishing.
pub fn expand_window_title_template(template: &str, vars: &WindowTitleVars) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            // An unclosed `{{` has no placeholder; keep it literally.
            out.push_str(&rest[start..]);
            return out;
        };
        let value = match after[..end].trim() {
            "title" => Some(&vars.title),
            "app_name" => Some(&vars.app_name),
            "pane_title" => Some(&vars.pane_title),
            "cwd" => Some(&vars.cwd),
            _ => None,
        };
        match value {
            Some(v) => out.push_str(v),
            None => {
                out.push_str("{{");
                out.push_str(&after[..end]);
                out.push_str("}}");
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// Names (filename without `.kdl`) of the theme files in `themes/`, sorted. An
/// absent or unreadable directory yields an empty list.
pub fn available_themes() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(themes_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension()?.to_str()? != "kdl" {
                return None;
            }
            path.file_stem()?.to_str().map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

/// Load and resolve `themes/<name>.kdl` into a full [`Theme`] (its `base` preset
/// with its `colors` layered on top), or `None` if the file is missing or
/// unparseable.
pub fn load_named_theme(name: &str) -> Option<Theme> {
    let text = std::fs::read_to_string(named_theme_path(name)).ok()?;
    parse_theme_file(&text)
}

/// The file path a named theme's colors are read from and saved to:
/// `themes/<name>.kdl`.
pub fn named_theme_path(name: &str) -> PathBuf {
    themes_dir().join(format!("{name}.kdl"))
}

/// Resolve a theme file's text into a [`Theme`]: start from the named `base`
/// preset (defaulting to dark) and apply its color overrides.
fn parse_theme_file(text: &str) -> Option<Theme> {
    let kdl: KdlThemeFile = kdl::de::from_str(text).ok()?;
    let mut theme = match kdl.base.as_deref() {
        Some("light") => Theme::light(),
        _ => Theme::dark(),
    };
    if let Some(colors) = kdl.colors {
        color_overrides_from_kdl(colors).apply(&mut theme);
    }
    Some(theme)
}

/// The user theme directory: `<config_dir>/themes`.
fn themes_dir() -> PathBuf {
    config_dir().join("themes")
}

/// Whether `name` is safe to use as a `themes/<name>.kdl` file stem: non-empty
/// and restricted to characters that can't escape `themes_dir()` (no `/`, `.`,
/// or other path-traversal characters).
pub fn is_valid_theme_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Write a new `themes/<name>.kdl` seeded with `theme`'s full resolved colors,
/// so the file is immediately usable as a starting point for hand-editing.
/// Errors if `name` is invalid or a theme with that name already exists (never
/// overwrites a file the user may have edited).
pub fn save_named_theme(name: &str, theme: &Theme) -> std::io::Result<()> {
    if !is_valid_theme_name(name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "theme name must be non-empty and contain only letters, numbers, - or _",
        ));
    }
    std::fs::create_dir_all(themes_dir())?;
    let path = named_theme_path(name);
    if path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("a theme named \"{name}\" already exists"),
        ));
    }
    std::fs::write(path, theme_kdl(theme))
}

/// Whether `bg` reads as a light surface color (perceptual luminance above
/// the midpoint), used to pick the closer built-in preset as a theme's `base`.
fn is_light_background(bg: ThemeRgb) -> bool {
    let luma = 0.299 * bg.r as f32 + 0.587 * bg.g as f32 + 0.114 * bg.b as f32;
    luma > 127.0
}

/// Serialize `theme`'s full resolved colors as theme-file KDL text: a `base`
/// matching the theme's overall brightness, plus a fully populated `colors`
/// block. `base` isn't purely informational: chrome colors outside the
/// `colors` schema (menu/tab surfaces) come from the base preset, not from
/// `colors`, so picking the closer preset keeps those parts consistent too.
fn theme_kdl(theme: &Theme) -> String {
    let base = if is_light_background(theme.background) {
        "light"
    } else {
        "dark"
    };
    let mut out = format!("base {}\n\ncolors {{\n", kdl_string(base));
    let scalars: [(&str, ThemeRgb); 11] = [
        ("background", theme.background),
        ("foreground", theme.foreground),
        ("cursor-bg", theme.cursor_bg),
        ("cursor-fg", theme.cursor_fg),
        ("scrollbar", theme.scrollbar),
        ("selection-bg", theme.selection_bg),
        ("selection-fg", theme.selection_fg),
        ("split", theme.divider),
        ("status-bar-border", theme.status_bar_border),
        ("visual-bell", theme.bell.unwrap_or(theme.cursor_bg)),
        ("window-border", theme.window_border),
    ];
    for (name, rgb) in scalars {
        out.push_str(&format!("    {name} {}\n", kdl_string(&rgb.to_hex())));
    }
    let ansi: [Option<ThemeRgb>; 8] = std::array::from_fn(|i| Some(theme.ansi[i]));
    let brights: [Option<ThemeRgb>; 8] = std::array::from_fn(|i| Some(theme.ansi[8 + i]));
    if let Some(line) = color_named_block_kdl("ansi", &ansi) {
        out.push_str(&line);
    }
    if let Some(line) = color_named_block_kdl("brights", &brights) {
        out.push_str(&line);
    }
    if !theme.indexed.is_empty() {
        out.push_str("    indexed {\n");
        for (index, rgb) in &theme.indexed {
            out.push_str(&format!(
                "        {} {}\n",
                kdl_string(&index.to_string()),
                kdl_string(&rgb.to_hex())
            ));
        }
        out.push_str("    }\n");
    }
    out.push_str("}\n");
    out
}

/// Persistent runtime state stored in `<state_dir>/state.json`.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct AppState {
    /// Previously executed palette queries, ordered most-recent first.
    #[serde(default)]
    pub palette_history: Vec<String>,
    /// Last known window dimensions in physical pixels.
    pub window_size: Option<(u32, u32)>,
}

/// Load state from `<state_dir>/state.json`, returning a default on any error.
pub fn load_state() -> AppState {
    let path = state_dir().join("state.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist state to `<state_dir>/state.json`, silently ignoring write errors.
pub fn save_state(state: &AppState) {
    let dir = state_dir();
    if std::fs::create_dir_all(&dir).is_ok() {
        if let Ok(json) = serde_json::to_string(state) {
            let _ = std::fs::write(dir.join("state.json"), json);
        }
    }
}

/// Parse a KDL boolean-ish string (`"true"`/`"false"`), falling back to `default`.
/// Quote `s` as a KDL string argument, escaping backslashes and double quotes.
fn kdl_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn kdl_bool(value: bool) -> &'static str {
    if value {
        "#true"
    } else {
        "#false"
    }
}

/// The 8 standard ANSI color names, in SGR order (index 0 = black, ...,
/// index 7 = white). `brights` reuses the same names for its own 8 slots.
const ANSI_COLOR_NAMES: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

/// A `colors` sub-block (`ansi`/`brights`) of named hex-color nodes (e.g.
/// `ansi { red "#c22727" }`), or `None` when every entry is unset.
fn color_named_block_kdl(name: &str, colors: &[Option<ThemeRgb>; 8]) -> Option<String> {
    if colors.iter().all(Option::is_none) {
        return None;
    }
    let mut out = format!("    {name} {{\n");
    for (color_name, value) in ANSI_COLOR_NAMES.iter().zip(colors) {
        if let Some(rgb) = value {
            out.push_str(&format!(
                "        {color_name} {}\n",
                kdl_string(&rgb.to_hex())
            ));
        }
    }
    out.push_str("    }\n");
    Some(out)
}

fn color_overrides_from_kdl(kdl: KdlColors) -> ColorOverrides {
    fn hex(s: Option<String>) -> Option<ThemeRgb> {
        s.and_then(|v| ThemeRgb::parse_hex(&v))
    }
    /// Look each ANSI color name up in `map`, in [`ANSI_COLOR_NAMES`] order, so
    /// a name absent from `map` leaves that slot unset rather than shifting the
    /// rest into the wrong position.
    fn hex_named(map: Option<HashMap<String, String>>) -> [Option<ThemeRgb>; 8] {
        let map = map.unwrap_or_default();
        let mut out = [None; 8];
        for (slot, name) in out.iter_mut().zip(ANSI_COLOR_NAMES) {
            *slot = map.get(name).and_then(|s| ThemeRgb::parse_hex(s));
        }
        out
    }

    ColorOverrides {
        background: hex(kdl.background),
        foreground: hex(kdl.foreground),
        cursor_bg: hex(kdl.cursor_bg),
        cursor_fg: hex(kdl.cursor_fg),
        scrollbar: hex(kdl.scrollbar),
        selection_bg: hex(kdl.selection_bg),
        selection_fg: hex(kdl.selection_fg),
        divider: hex(kdl.split),
        status_bar_border: hex(kdl.status_bar_border),
        window_border: hex(kdl.window_border),
        bell: hex(kdl.visual_bell),
        ansi: hex_named(kdl.ansi),
        brights: hex_named(kdl.brights),
        indexed: kdl
            .indexed
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(k, v)| {
                let index = k.parse::<u8>().ok()?;
                let color = ThemeRgb::parse_hex(&v)?;
                Some((index, color))
            })
            .collect(),
    }
}

/// Interpret a `window-controls-side` config value: `"left"` (the default)
/// or `"right"`. Unknown values fall back to the default left side.
fn controls_side_from_value(value: &str) -> ControlsSide {
    match value.trim().to_ascii_lowercase().as_str() {
        "right" => ControlsSide::Right,
        _ => ControlsSide::Left,
    }
}

/// The canonical config value for a [`ControlsSide`] (round-trips through
/// [`controls_side_from_value`]).
fn controls_side_as_value(side: ControlsSide) -> &'static str {
    match side {
        ControlsSide::Left => "left",
        ControlsSide::Right => "right",
    }
}

/// Apply the `security` block on top of the deny-by-default policy.
///
/// An unparseable tier keeps the default rather than failing the whole config:
/// a typo in `block-max-trust` must never silently *raise* the ceiling, and
/// dropping the entire settings file over one bad word would be worse.
fn security_config_from_kdl(kdl: KdlSecurity) -> SecurityConfig {
    let defaults = SecurityConfig::default();
    SecurityConfig {
        block_max_trust: kdl
            .block_max_trust
            .as_deref()
            .and_then(|value| value.parse().ok())
            .unwrap_or(defaults.block_max_trust),
        block_remote_assets: kdl
            .block_remote_assets
            .unwrap_or(defaults.block_remote_assets),
    }
}

/// Apply the `cursor` block on top of the default per-mode shapes; any unset
/// entry keeps its default. Unknown shape strings fall back to `Block` via
/// [`CursorShape::from_value`].
fn cursor_config_from_kdl(kdl: KdlCursor) -> CursorConfig {
    let defaults = CursorConfig::default();
    CursorConfig {
        blink: kdl.blink.unwrap_or(defaults.blink),
        block_focus: kdl
            .block_focus
            .as_deref()
            .map(CursorShape::from_value)
            .unwrap_or(defaults.block_focus),
        hide_in_inactive: kdl.hide_in_inactive.unwrap_or(defaults.hide_in_inactive),
        insert: kdl
            .insert
            .as_deref()
            .map(CursorShape::from_value)
            .unwrap_or(defaults.insert),
        normal: kdl
            .normal
            .as_deref()
            .map(CursorShape::from_value)
            .unwrap_or(defaults.normal),
        visual: kdl
            .visual
            .as_deref()
            .map(CursorShape::from_value)
            .unwrap_or(defaults.visual),
    }
}

/// Parse a standalone `keybindings.kdl` into the mode -> (key -> action) map. An empty
/// or unparseable file yields an empty map (callers keep their defaults).
fn parse_keys(text: &str) -> HashMap<String, HashMap<String, String>> {
    if text.trim().is_empty() {
        return HashMap::new();
    }
    kdl::de::from_str::<HashMap<String, HashMap<String, String>>>(text).unwrap_or_default()
}

/// The configuration directory: `%APPDATA%\winter-term` on Windows;
/// `$XDG_CONFIG_HOME/winter-term` or `~/.config/winter-term` elsewhere.
fn config_dir() -> PathBuf {
    #[cfg(windows)]
    {
        match std::env::var("APPDATA") {
            Ok(appdata) => PathBuf::from(appdata).join("winter-term"),
            Err(_) => PathBuf::from("."),
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            PathBuf::from(xdg).join("winter-term")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".config/winter-term")
        } else {
            PathBuf::from(".")
        }
    }
}

/// The state directory: `%LOCALAPPDATA%\winter-term` on Windows (machine-local,
/// non-roaming — matching `state.json`'s regenerable, per-machine nature);
/// `$XDG_STATE_HOME/winter-term` or `~/.local/state/winter-term` elsewhere.
fn state_dir() -> PathBuf {
    #[cfg(windows)]
    {
        match std::env::var("LOCALAPPDATA") {
            Ok(local_appdata) => PathBuf::from(local_appdata).join("winter-term"),
            Err(_) => PathBuf::from("."),
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
            PathBuf::from(xdg).join("winter-term")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".local/state/winter-term")
        } else {
            PathBuf::from(".")
        }
    }
}

/// One problem found in a config file: its 1-based `line` and a human `message`.
struct ConfigProblem {
    line: usize,
    message: String,
}

/// Pull the individual problems out of a KDL parse error so each can be
/// reported on its own line. The KDL parser collects every issue as a "related"
/// diagnostic with span info; this surfaces them with 1-based line numbers
/// located against `text`. Falls back to the top-level message when no related
/// diagnostics are available.
fn config_problems(err: &kdl::de::Error, text: &str) -> Vec<ConfigProblem> {
    use miette::Diagnostic;

    let related: Vec<&dyn Diagnostic> = err
        .related()
        .map(|problems| problems.collect())
        .unwrap_or_default();

    // No related diagnostics means the file failed to tokenize at all; report the
    // whole error as a single problem rather than dropping it silently.
    if related.is_empty() {
        return vec![ConfigProblem {
            line: 1,
            message: err.to_string(),
        }];
    }

    related
        .into_iter()
        .map(|problem| {
            let offset = problem
                .labels()
                .and_then(|mut labels| labels.next())
                .map(|label| label.offset())
                .unwrap_or(0);
            ConfigProblem {
                line: line_number(text, offset),
                message: problem.to_string(),
            }
        })
        .collect()
}

/// The 1-based line number containing byte `offset` within `text`.
fn line_number(text: &str, offset: usize) -> usize {
    let end = offset.min(text.len());
    text.as_bytes()[..end]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    // Tests build configs by mutating fields off `default()` for readability,
    // which is clearer here than large struct literals.
    #![allow(clippy::field_reassign_with_default)]

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
    fn test_config_problems_report_unknown_key() {
        let text = "theme \"dark\"\nfont-weigth \"300\"\n";
        let err = kdl::de::from_str::<KdlConfig>(text)
            .err()
            .expect("unknown key should fail to parse");
        let problems = config_problems(&err, text);
        assert!(
            !problems.is_empty(),
            "unknown key produces at least one problem"
        );
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
    fn test_expand_window_title_template() {
        let vars = WindowTitleVars {
            title: "Terminal 1".into(),
            app_name: "butterfly".into(),
            pane_title: "butterfly — notes.md".into(),
            cwd: "~/N/notes".into(),
        };
        // Every placeholder, with and without inner spaces.
        assert_eq!(
            expand_window_title_template("{{title}}", &vars),
            "Terminal 1"
        );
        assert_eq!(
            expand_window_title_template("{{ app_name }}: {{ pane_title }}", &vars),
            "butterfly: butterfly — notes.md"
        );
        assert_eq!(
            expand_window_title_template("{{ cwd }}", &vars),
            "~/N/notes"
        );
        // Unknown placeholders and unclosed braces stay literal.
        assert_eq!(
            expand_window_title_template("{{ bogus }}", &vars),
            "{{ bogus }}"
        );
        assert_eq!(expand_window_title_template("a {{ b", &vars), "a {{ b");
        // Text without placeholders passes through unchanged.
        assert_eq!(expand_window_title_template("Winter", &vars), "Winter");
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
        "Ctrl-Space" "toggle_mode"
    }
}
"#,
        );
        assert_eq!(config.keybindings.len(), 2);
        let normal = config.keybindings.get("normal").unwrap();
        assert_eq!(normal.get("j"), Some(&"focus_down".to_string()));
        assert_eq!(normal.get("k"), Some(&"focus_up".to_string()));
        let insert = config.keybindings.get("insert").unwrap();
        assert_eq!(insert.get("Ctrl-Space"), Some(&"toggle_mode".to_string()));
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
    "Ctrl-w v" "split_vertical"
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
                .and_then(|m| m.get("Ctrl-w v")),
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
    fn test_parse_status_bar_icons() {
        let config = Config::parse(
            r#"
status-bar {
    normal-icon "N"
    insert-icon "I"
    block-icon "B"
}
"#,
        );
        assert_eq!(config.status_bar.icons.normal, "N");
        assert_eq!(config.status_bar.icons.insert, "I");
        assert_eq!(config.status_bar.icons.block, "B");
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
        assert_eq!(config.cursor.block_focus, CursorShape::Bar);
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
        assert_eq!(config.cursor.block_focus, CursorShape::Bar);
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
        assert_eq!(parsed.cursor.block_focus, CursorShape::Bar);
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
    fn test_parse_theme_file_applies_base_and_colors() {
        let theme = parse_theme_file(
            r##"
base "light"
colors {
    background "#282a36"
    foreground "#f8f8f2"
}
"##,
        )
        .expect("theme file parses");
        // Base preset is light, then the two colors override it.
        assert_eq!(theme.background, ThemeRgb::parse_hex("#282a36").unwrap());
        assert_eq!(theme.foreground, ThemeRgb::parse_hex("#f8f8f2").unwrap());
        // An unspecified color keeps the light base.
        assert_eq!(theme.cursor_bg, Theme::light().cursor_bg);
    }

    #[test]
    fn test_parse_theme_file_defaults_base_to_dark() {
        let theme = parse_theme_file("colors {\n    background \"#000000\"\n}").unwrap();
        assert_eq!(theme.cursor_bg, Theme::dark().cursor_bg);
    }

    #[test]
    fn test_is_valid_theme_name_accepts_and_rejects() {
        assert!(is_valid_theme_name("dracula"));
        assert!(is_valid_theme_name("Solarized-Dark_2"));
        assert!(!is_valid_theme_name(""));
        assert!(!is_valid_theme_name("../evil"));
        assert!(!is_valid_theme_name("has/slash"));
        assert!(!is_valid_theme_name("has.dot"));
        assert!(!is_valid_theme_name("has space"));
    }

    #[test]
    fn test_is_light_background() {
        assert!(!is_light_background(Theme::dark().background));
        assert!(is_light_background(Theme::light().background));
    }

    #[test]
    fn test_theme_kdl_round_trips_through_parse_theme_file() {
        for theme in [Theme::dark(), Theme::light()] {
            let restored =
                parse_theme_file(&theme_kdl(&theme)).expect("generated theme kdl parses");
            assert_eq!(restored, theme);
        }
    }

    #[test]
    fn test_save_named_theme_rejects_invalid_name() {
        // Validation happens before any filesystem access, so this is safe to
        // run without touching the real theme directory.
        let err = save_named_theme("../evil", &Theme::dark()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_to_kdl_roundtrips_color_overrides() {
        let mut config = Config::default();
        config.colors.background = ThemeRgb::parse_hex("#1a1a2e");
        config.colors.cursor_bg = ThemeRgb::parse_hex("#52ad70");
        config.colors.window_border = ThemeRgb::parse_hex("#123456");
        config.colors.ansi[0] = ThemeRgb::parse_hex("#000000");
        config.colors.indexed = vec![(136, ThemeRgb::parse_hex("#af8700").unwrap())];

        let parsed = Config::parse(&config.to_kdl());
        assert_eq!(parsed.colors.background, ThemeRgb::parse_hex("#1a1a2e"));
        assert_eq!(parsed.colors.cursor_bg, ThemeRgb::parse_hex("#52ad70"));
        assert_eq!(parsed.colors.window_border, ThemeRgb::parse_hex("#123456"));
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
    fn test_status_bar_visibility_defaults_on_and_parses_off() {
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
    fn test_security_defaults_deny_scripting_and_network() {
        let c = Config::default();
        assert_eq!(c.security.block_max_trust, TrustTier::Restricted);
        assert!(!c.security.block_remote_assets);
        assert_eq!(
            Config::parse("").security.block_max_trust,
            TrustTier::Restricted,
            "a config file with no security block must not widen the policy"
        );
    }

    #[test]
    fn test_security_parses_and_round_trips() {
        let raised = Config::parse(
            r#"security {
                block-max-trust "trusted"
                block-remote-assets #true
            }"#,
        );
        assert_eq!(raised.security.block_max_trust, TrustTier::Trusted);
        assert!(raised.security.block_remote_assets);

        let restored = Config::parse(&raised.to_kdl());
        assert_eq!(restored.security.block_max_trust, TrustTier::Trusted);
        assert!(restored.security.block_remote_assets);
    }

    #[test]
    fn test_unparseable_trust_tier_keeps_the_safe_default() {
        // A typo must never be the thing that widens the ceiling.
        let c = Config::parse(r#"security { block-max-trust "admin" }"#);
        assert_eq!(c.security.block_max_trust, TrustTier::Restricted);
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
    fn test_parse_checked_reports_nothing_for_a_good_file() {
        let (_, error) = Config::parse_checked("font-size 17");
        assert!(error.is_none());
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
}
