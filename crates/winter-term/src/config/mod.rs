//! KDL configuration parser: theme, fonts, keybindings, and appearance.
//!
//! Configuration lives in `~/.config/winter-term/` (or `$XDG_CONFIG_HOME/winter-term`;
//! `%APPDATA%\winter-term` on Windows) split across two files:
//! - `settings.kdl`: appearance and behavior (theme, fonts, colors, menu style,
//!   status bar).
//! - `keybindings.kdl`: keybindings, as top-level mode blocks.
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
//! window-title-template "{{ name }} — Winter Term"
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
//!     "C-S-q" "close_pane"
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

pub(crate) mod diagnostics;
mod read;
pub(crate) mod schema;
pub(crate) mod state;
pub(crate) mod theme;
pub(crate) mod title;
pub(crate) mod watch;
mod write;


pub use winter_render::ColorOverrides;

// Keep the flat `crate::config::<name>` paths the rest of the app already uses.
pub use read::todo_keywords_from_names;
use read::builtin_settings;
pub(crate) use state::{
    are_tasks_trusted, config_dir, load_state, remember_compile_command,
    remembered_compile_command, save_state, trust_tasks, AppState,
};
pub(crate) use theme::{available_themes, is_valid_theme_name, load_named_theme, save_named_theme};
pub(crate) use title::{expand_window_title_template, WindowTitleVars};

use std::collections::HashMap;

use winter_render::{ControlsSide, MenuStyle};

use winter_core::CursorShape;

use crate::model::paint::GutterSide;

// ========================================================================
// Constants
// ========================================================================

/// Every built-in completion back-end a setting may name. `lsp` is answered
/// by a language server once one is started for the project.
pub(crate) const COMPLETION_BACKENDS: &[&str] = &[
    "keywords", "latex", "lsp", "markdown", "paths", "snippets", "words",
];

/// How many screen rows an editor keeps above and below its cursor's as the
/// view follows it, unless `scroll-margin` says otherwise.
pub(crate) const DEFAULT_SCROLL_MARGIN: usize = 5;

/// The most `scroll-margin` may ask for; a pane holds it to under half its
/// height besides.
pub(crate) const MAX_SCROLL_MARGIN: usize = 50;

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
        Config::default().status_bar
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
    /// What an extension's segment, by its full id, was last told to say.
    Extension(&'static str),
    /// Z.ai's quota, under its logo.
    Zai,
}

impl StatusSegmentKind {
    /// Every built-in segment, in the order the right side shows them when
    /// `settings.kdl` names none.
    pub const ALL: [Self; 4] = [Self::Antigravity, Self::Claude, Self::Zai, Self::Editor];

    /// Every segment, in the order the right side shows them when
    /// `settings.kdl` names none: the built-in ones, the extensions' just
    /// before the editor's.
    pub fn defaults() -> Vec<Self> {
        let (quotas, editor) = Self::ALL.split_at(Self::ALL.len() - 1);
        let extensions = crate::extensions::registry()
            .segments()
            .iter()
            .map(|segment| Self::Extension(segment.id.as_str()));
        quotas.iter().copied().chain(extensions).chain(editor.iter().copied()).collect()
    }

    /// The segment `settings.kdl` calls `name`, if it is one.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "antigravity" => Some(Self::Antigravity),
            "claude" => Some(Self::Claude),
            "tool" => Some(Self::Editor),
            "zai" => Some(Self::Zai),
            other => crate::extensions::segment(other).map(Self::Extension),
        }
    }

    /// What `settings.kdl` calls the segment.
    pub fn name(self) -> &'static str {
        match self {
            Self::Antigravity => "antigravity",
            Self::Claude => "claude",
            Self::Editor => "tool",
            Self::Extension(id) => id,
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
        Config::default().notifications
    }
}

/// How the Compile tab builds a LaTeX document, and when.
#[derive(Clone, Debug)]
pub struct CompileConfig {
    /// Whether the compilation view appears in the dock along the bottom.
    pub dock: bool,
    /// Whether a failed build opens its first error by itself.
    pub jump_to_error: bool,
    /// The command a document's root is built with, its file name added.
    pub latex_command: String,
    /// Whether saving a `.tex` file builds the document it belongs to.
    pub latex_on_save: bool,
}

impl Default for CompileConfig {
    fn default() -> Self {
        Config::default().compile
    }
}

/// How the text editor completes what is typed.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionConfig {
    /// Whether the popup opens by itself while a word is typed, once it is
    /// [`Self::min_chars`] long, rather than only on `C-SPC`.
    pub auto: bool,
    /// The back-ends each language asks, in order, by the name settings give
    /// the language; a language not named asks its defaults.
    pub backends: std::collections::BTreeMap<String, Vec<String>>,
    /// How long typing pauses before the popup opens by itself, so it does
    /// not flicker up under every word typed straight through. `C-SPC`
    /// opens it at once, and an open popup narrows at once.
    pub delay: std::time::Duration,
    /// How long a word is before the popup opens by itself.
    pub min_chars: usize,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        Config::default().completion
    }
}

/// GitHub Copilot's suggestions in the text editor, through its language
/// server.
#[derive(Clone, Debug, PartialEq)]
pub struct CopilotConfig {
    /// Whether ghost text is offered as you type.
    pub completions: bool,
    /// Whether Copilot runs at all: started the first time a file is typed
    /// into, with the sign-in Copilot keeps on this machine.
    pub enable: bool,
    /// Whether the next edit is predicted after a change, for `Tab` to make.
    pub next_edit: bool,
    /// The `copilot-language-server` to run; empty to look for one.
    pub server_path: String,
}

impl Default for CopilotConfig {
    fn default() -> Self {
        Config::default().copilot
    }
}

/// Which language servers start without being asked, and which program
/// serves a language where the one Winter knows is not wanted.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LspConfig {
    /// The languages whose server starts as soon as a file of theirs is
    /// open; none by default, so a server runs only when asked for.
    pub auto_start: Vec<String>,
    /// The command (program and arguments) serving each language named.
    pub servers: std::collections::BTreeMap<String, Vec<String>>,
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
    /// How far the text editor indents the rows a wrapped line continues on.
    pub editor_wrap_indent: crate::model::paint::WrapIndent,
    /// When the text editor hides markup's markers (`**`, `\textbf{`, ...).
    pub editor_markup_markers: crate::model::paint::MarkupMarkers,
    /// User keybindings, keyed by mode and then by chord.
    pub keybindings: HashMap<String, HashMap<String, String>>,
    /// Why `keybindings.kdl` was ignored, when it did not parse, with the
    /// line of its first problem. Kept apart from a rejected settings.kdl: the
    /// settings themselves still apply. While it is set, [`Self::keybindings`]
    /// holds only what `settings.kdl` binds, not the file's bindings, so a
    /// reload should keep the keymap already in force rather than rebuild it
    /// from these.
    pub keybindings_error: Option<String>,
    /// Something in `settings.kdl` read but not understood (an unknown
    /// status-bar segment, a misspelled back-end): reported, while the rest
    /// of the file applies. Kept apart from a file that does not parse, which
    /// is refused whole.
    pub settings_warning: Option<String>,
    /// Enable OpenType ligatures in the font renderer. Defaults to `false`:
    /// like most terminals, winter keeps each cell's glyph at its nominal
    /// position so contextual shaping never breaks the monospace grid (e.g.
    /// raising the middle asterisk of `***`). Set `ligatures #true` to opt in.
    pub ligatures: bool,
    /// Tabbar menu presentation: a modern hamburger dropdown or a classic menubar.
    pub menu_style: MenuStyle,
    /// How notices are shown.
    pub notifications: NotificationsConfig,
    /// How the Compile tab builds LaTeX.
    pub compile: CompileConfig,
    /// How the text editor completes what is typed.
    pub completion: CompletionConfig,
    /// The language servers.
    pub lsp: LspConfig,
    /// GitHub Copilot's suggestions.
    pub copilot: CopilotConfig,
    /// Window opacity, from 0.0 (transparent) to 1.0 (opaque).
    pub opacity: f32,
    /// Draw an underline under fuzzy-matched characters in the command palette.
    pub palette_match_underline: bool,
    /// Thickness of the line drawn between adjacent panes, in logical pixels. Default: 2.
    pub pane_border_width: f32,
    /// How tall the field typed into is in every popup (a picker, the find
    /// box, a question), in rows of the terminal's font. Default: 1.5.
    pub popup_input_height: f32,
    /// Right-click pastes from the clipboard instead of opening the context menu.
    pub paste_on_right_click: bool,
    /// Which line-editor bindings the shell answers to, for Vim operators on
    /// the prompt line. Default: [`PromptEditBindings::Emacs`].
    pub prompt_edit_bindings: PromptEditBindings,
    /// Bracket glyphs depth-colored by nesting depth. Default: `false`.
    pub rainbow_parens: bool,
    /// Save the tab/pane layout on exit and reopen it on the next launch. Default: `false`.
    pub restore_session: bool,
    /// How many screen rows an editor keeps above and below the cursor's as
    /// the view scrolls after it, `zt` and `zb` leave, and `H` and `L` stop
    /// short of, like Vim's `scrolloff`. Default: [`DEFAULT_SCROLL_MARGIN`].
    pub scroll_margin: usize,
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
    /// The words the Todo list finds, each as a whole word in its own case.
    pub todo_keywords: Vec<String>,
    /// Underline `http://`/`https://` URLs (auto-detected and OSC 8
    /// hyperlinks alike). Default: `false`.
    pub url_underline: bool,
    /// Which edge carries the minimize/maximize/close buttons (the hamburger
    /// button sits on the opposite edge).
    pub window_controls_side: ControlsSide,
    /// Window title template. `{{ ... }}` placeholders expand to the running
    /// program's title/name/cwd (see [`WindowTitleVars`]); the rest is literal
    /// text. Default: `{{ name }} - {{ location }} - Winter`.
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
        Config::default().cursor
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
    /// The settings `builtin/settings.kdl` gives.
    fn default() -> Self {
        builtin_settings()
    }
}

impl Config {
    /// Settings that set nothing: every flag off, every number zero, every
    /// list empty but the status bar's (which means all of its segments).
    /// What the built-in settings file is read over, so every value a
    /// setting has by default is the file's.
    fn unset() -> Self {
        Self {
            colors: ColorOverrides::default(),
            cursor: CursorConfig {
                blink: false,
                hide_in_inactive: false,
                insert: CursorShape::default(),
                normal: CursorShape::default(),
                visual: CursorShape::default(),
            },
            dim_inactive: false,
            font_family: None,
            font_size: 0.0,
            font_weight: None,
            font_weight_bold: None,
            gutter_side: GutterSide::default(),
            editor_wrap_indent: crate::model::paint::WrapIndent::default(),
            editor_markup_markers: crate::model::paint::MarkupMarkers::default(),
            keybindings: HashMap::new(),
            keybindings_error: None,
            settings_warning: None,
            ligatures: false,
            clipboard_read: false,
            menu_style: MenuStyle::default(),
            notifications: NotificationsConfig { toasts: false },
            compile: CompileConfig {
                dock: false,
                jump_to_error: false,
                latex_command: String::new(),
                latex_on_save: false,
            },
            completion: CompletionConfig {
                auto: false,
                backends: std::collections::BTreeMap::new(),
                delay: std::time::Duration::ZERO,
                min_chars: 0,
            },
            lsp: LspConfig::default(),
            copilot: CopilotConfig {
                completions: false,
                enable: false,
                next_edit: false,
                server_path: String::new(),
            },
            opacity: 0.0,
            palette_match_underline: false,
            pane_border_width: 0.0,
            popup_input_height: 0.0,
            paste_on_right_click: false,
            prompt_edit_bindings: PromptEditBindings::default(),
            rainbow_parens: false,
            restore_session: false,
            scroll_margin: DEFAULT_SCROLL_MARGIN,
            scrollback_lines: None,
            sentence_highlight: false,
            shell: None,
            shell_windows: None,
            shell_linux: None,
            shell_macos: None,
            status_bar: StatusBarConfig {
                enabled: false,
                priority: Vec::new(),
                segments: StatusSegmentKind::defaults(),
                show_mode: false,
            },
            theme: ThemeSetting::default(),
            title_bar_style: TitleBarStyle::default(),
            todo_keywords: Vec::new(),
            url_underline: false,
            window_controls_side: ControlsSide::default(),
            window_title_template: String::new(),
            wrap_indent: false,
            wrap_words: false,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Every completion back-end a setting may name: the built-in ones, then
/// each extension's by its full id.
pub(crate) fn completion_backends() -> Vec<&'static str> {
    let extensions = crate::extensions::registry().completions().iter().map(|source| source.id.as_str());
    COMPLETION_BACKENDS.iter().copied().chain(extensions).collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;
