//! Reading `settings.kdl` and `keybindings.kdl` into a [`Config`], and what to
//! say about the names in them that are none Winter knows.

use std::sync::LazyLock;

use winter_render::{ColorOverrides, MenuStyle};

use crate::model::paint::{GutterSide, MarkupMarkers, WrapIndent};

use super::diagnostics::config_problems;
use super::schema::{controls_side_from_value, cursor_config_from_kdl, parse_keys, KdlConfig};
use super::state::config_dir;
use super::{
    CompileConfig, CompletionConfig, Config, CopilotConfig, LspConfig, NotificationsConfig,
    PromptEditBindings, StatusBarConfig, StatusSegmentKind, ThemeSetting, TitleBarStyle,
    MAX_SCROLL_MARGIN,
};

// ========================================================================
// Constants
// ========================================================================

/// Winter's own settings: the defaults, which every settings file is read
/// over. Also installed as the reference users copy from.
const BUILTIN_SETTINGS_KDL: &str = include_str!("../../builtin/settings.kdl");

/// The least background opacity a setting can ask for: below it the window
/// would be all but invisible.
const MIN_OPACITY: f32 = 0.1;

/// The thinnest line drawn between split panes, in logical pixels.
const MIN_PANE_BORDER_WIDTH: f32 = 1.0;

/// The built-in settings, read once over settings that set nothing.
static BUILTIN: LazyLock<Config> = LazyLock::new(|| {
    let kdl = kdl::de::from_str(BUILTIN_SETTINGS_KDL).expect("the built-in settings parse");
    read_settings(kdl, &Config::unset()).0
});

// ========================================================================
// Config
// ========================================================================

impl Config {
    /// Load configuration from `~/.config/winter-term/`, also returning a
    /// one-line summary when a config file was rejected.
    ///
    /// Settings come from `settings.kdl` and keybindings from
    /// `keybindings.kdl`; with neither, defaults.
    ///
    /// The diagnostic is returned rather than only logged because the GUI is
    /// usually launched from a desktop menu with no terminal attached, so a
    /// message on stderr is invisible and the user just sees their settings
    /// not applying.
    pub fn load_checked() -> (Self, Option<String>) {
        let dir = config_dir();
        let settings_path = dir.join("settings.kdl");
        let keys_path = dir.join("keybindings.kdl");

        let settings = std::fs::read_to_string(&settings_path).unwrap_or_default();
        let keys = std::fs::read_to_string(&keys_path).unwrap_or_default();
        Self::parse_with_keys_checked(&settings, &keys)
    }

    /// Parse settings (`settings.kdl`) and keybindings (`keybindings.kdl`) from separate
    /// sources.
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

        let base = builtin_settings();
        let (mut config, unknown_segments) = read_settings(kdl, &base);
        // Not a refusal: the file applies, and the slip is reported beside it.
        config.settings_warning = unknown_syntax_warning(&config.colors.unknown_syntax)
            .or_else(|| unknown_segment_warning(&unknown_segments))
            .or_else(|| unknown_backend_warning(&config.completion));
        (config, None)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The settings `builtin/settings.kdl` gives: every other settings file's
/// base, and what [`Config::default`] is.
pub(super) fn builtin_settings() -> Config {
    BUILTIN.clone()
}

/// Read `kdl` over `base`: a setting it leaves out, or gives a value that
/// cannot be used, keeps `base`'s. Also returns the status-bar segment names
/// that are none.
fn read_settings(kdl: KdlConfig, base: &Config) -> (Config, Vec<String>) {
    let mut unknown_segments = Vec::new();
    let status_bar = match kdl.status_bar {
        Some(sb) => {
            let mut segments_named = |names: Option<Vec<String>>, base: &[StatusSegmentKind]| {
                names.map_or_else(
                    || base.to_vec(),
                    |names| status_segments_from_names(names, &mut unknown_segments),
                )
            };
            StatusBarConfig {
                enabled: sb.show.unwrap_or(base.status_bar.enabled),
                priority: segments_named(sb.priority, &base.status_bar.priority),
                segments: segments_named(sb.segments, &base.status_bar.segments),
                show_mode: sb.show_mode.unwrap_or(base.status_bar.show_mode),
            }
        }
        None => base.status_bar.clone(),
    };
    let completion = match kdl.completion {
        Some(block) => CompletionConfig {
            auto: block.auto.unwrap_or(base.completion.auto),
            backends: block.backends.map_or_else(
                || base.completion.backends.clone(),
                |backends| {
                    backends
                        .into_iter()
                        .map(|(language, names)| (language, names.0))
                        .collect()
                },
            ),
            delay: block
                .delay_ms
                .map_or(base.completion.delay, std::time::Duration::from_millis),
            min_chars: block
                .min_chars
                .map_or(base.completion.min_chars, |chars| (chars as usize).max(1)),
        },
        None => base.completion.clone(),
    };
    let copilot = match kdl.copilot {
        Some(block) => CopilotConfig {
            completions: block.completions.unwrap_or(base.copilot.completions),
            enable: block.enable.unwrap_or(base.copilot.enable),
            next_edit: block.next_edit.unwrap_or(base.copilot.next_edit),
            server_path: block.server_path.map_or_else(
                || base.copilot.server_path.clone(),
                |path| path.trim().to_string(),
            ),
        },
        None => base.copilot.clone(),
    };
    let lsp = match kdl.lsp {
        Some(block) => LspConfig {
            auto_start: block
                .auto_start
                .map_or_else(|| base.lsp.auto_start.clone(), |names| names.0),
            servers: block.servers.map_or_else(
                || base.lsp.servers.clone(),
                |servers| {
                    servers
                        .into_iter()
                        .map(|(language, command)| (language, command.0))
                        .collect()
                },
            ),
        },
        None => base.lsp.clone(),
    };
    let compile = match kdl.compile {
        Some(block) => CompileConfig {
            dock: block.dock.unwrap_or(base.compile.dock),
            jump_to_error: block.jump_to_error.unwrap_or(base.compile.jump_to_error),
            latex_command: block
                .latex_command
                .filter(|command| !command.trim().is_empty())
                .unwrap_or_else(|| base.compile.latex_command.clone()),
            latex_on_save: block.latex_on_save.unwrap_or(base.compile.latex_on_save),
        },
        None => base.compile.clone(),
    };
    let named = |value: Option<String>, base: &Option<String>| {
        value.filter(|s| !s.trim().is_empty()).or_else(|| base.clone())
    };
    let config = Config {
        colors: kdl
            .colors
            .map_or_else(|| base.colors.clone(), ColorOverrides::from_kdl),
        cursor: kdl
            .cursor
            .map_or(base.cursor, |cursor| cursor_config_from_kdl(cursor, &base.cursor)),
        dim_inactive: kdl.dim_inactive.unwrap_or(base.dim_inactive),
        font_family: named(kdl.font, &base.font_family),
        font_size: kdl
            .font_size
            .map_or(base.font_size, winter_render::clamp_font_size),
        font_weight: named(kdl.font_weight, &base.font_weight),
        font_weight_bold: named(kdl.font_weight_bold, &base.font_weight_bold),
        editor_wrap_indent: kdl
            .editor_wrap_indent
            .as_deref()
            .map_or(base.editor_wrap_indent, WrapIndent::from_value),
        editor_markup_markers: kdl
            .editor_markup_markers
            .as_deref()
            .map_or(base.editor_markup_markers, MarkupMarkers::from_value),
        gutter_side: kdl
            .gutter_side
            .as_deref()
            .map_or(base.gutter_side, GutterSide::from_value),
        keybindings: base.keybindings.clone(),
        keybindings_error: None,
        settings_warning: None,
        menu_style: kdl.menu_style.as_deref().map_or(base.menu_style, |s| match s {
            "classic" => MenuStyle::Classic,
            _ => MenuStyle::Modern,
        }),
        completion,
        copilot,
        lsp,
        compile,
        notifications: kdl.notifications.map_or_else(
            || base.notifications.clone(),
            |block| NotificationsConfig {
                toasts: block.toasts.unwrap_or(base.notifications.toasts),
            },
        ),
        // KDL reads `#nan` and `#inf` as numbers, and `clamp` lets NaN
        // through: such a value keeps the setting's base instead.
        opacity: kdl
            .opacity
            .filter(|opacity| opacity.is_finite())
            .map_or(base.opacity, |opacity| opacity.clamp(MIN_OPACITY, 1.0)),
        ligatures: kdl.ligatures.unwrap_or(base.ligatures),
        clipboard_read: kdl.clipboard_read.unwrap_or(base.clipboard_read),
        palette_match_underline: kdl
            .palette_match_underline
            .unwrap_or(base.palette_match_underline),
        pane_border_width: kdl
            .pane_border_width
            .filter(|width| width.is_finite())
            .map_or(base.pane_border_width, |w| w.max(MIN_PANE_BORDER_WIDTH)),
        popup_input_height: kdl.popup_input_height.filter(|rows| rows.is_finite()).map_or(base.popup_input_height, |rows| {
            let (least, most) = winter_render::POPUP_INPUT_HEIGHT_RANGE;
            rows.clamp(least, most)
        }),
        paste_on_right_click: kdl.paste_on_right_click.unwrap_or(base.paste_on_right_click),
        prompt_edit_bindings: kdl
            .prompt_edit_bindings
            .as_deref()
            .map_or(base.prompt_edit_bindings, PromptEditBindings::from_value),
        rainbow_parens: kdl.rainbow_parens.unwrap_or(base.rainbow_parens),
        restore_session: kdl.restore_session.unwrap_or(base.restore_session),
        scroll_margin: kdl
            .scroll_margin
            .map_or(base.scroll_margin, |rows| (rows as usize).min(MAX_SCROLL_MARGIN)),
        scrollback_lines: kdl
            .scrollback_lines
            .map(|n| n as usize)
            .filter(|&n| n > 0)
            .or(base.scrollback_lines),
        sentence_highlight: kdl.sentence_highlight.unwrap_or(base.sentence_highlight),
        shell: named(kdl.shell, &base.shell),
        shell_windows: named(kdl.shell_windows, &base.shell_windows),
        shell_linux: named(kdl.shell_linux, &base.shell_linux),
        shell_macos: named(kdl.shell_macos, &base.shell_macos),
        status_bar,
        theme: kdl
            .theme
            .as_deref()
            .map_or_else(|| base.theme.clone(), ThemeSetting::from_value),
        title_bar_style: kdl
            .title_bar_style
            .as_deref()
            .map_or(base.title_bar_style, TitleBarStyle::from_value),
        todo_keywords: kdl
            .todo_keywords
            .map(keywords_named)
            .filter(|keywords| !keywords.is_empty())
            .unwrap_or_else(|| base.todo_keywords.clone()),
        url_underline: kdl.url_underline.unwrap_or(base.url_underline),
        window_controls_side: kdl
            .window_controls_side
            .as_deref()
            .map_or(base.window_controls_side, controls_side_from_value),
        window_title_template: kdl
            .window_title_template
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| base.window_title_template.clone()),
        wrap_indent: kdl.wrap_indent.unwrap_or(base.wrap_indent),
        wrap_words: kdl.wrap_words.unwrap_or(base.wrap_words),
    };
    (config, unknown_segments)
}

/// What to say about `syntax` colors named by no scope of the theme's table,
/// which would otherwise color nothing without a word. `Editor: Show Scopes
/// at Cursor` names the scope a character is in.
pub(super) fn unknown_syntax_warning(names: &[String]) -> Option<String> {
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

/// What to say about a completion back-end named in `backends` that there is
/// none of, which would otherwise offer nothing without a word.
pub(super) fn unknown_backend_warning(completion: &CompletionConfig) -> Option<String> {
    let known = super::completion_backends();
    let (language, name) = completion.backends.iter().find_map(|(language, names)| {
        names
            .iter()
            .find(|name| !known.contains(&name.as_str()))
            .map(|name| (language, name))
    })?;
    Some(format!(
        "settings.kdl: completion back-end {name} (for {language}) is none of {}",
        known.join(", ")
    ))
}

/// The keywords `todo-keywords` names, trimmed, blanks and repeats dropped,
/// in their order. A list left with none falls back to the defaults, since a
/// Todo list that can find nothing is never what was meant.
pub fn todo_keywords_from_names(names: Vec<String>) -> Vec<String> {
    let keywords = keywords_named(names);
    match keywords.is_empty() {
        true => Config::default().todo_keywords,
        false => keywords,
    }
}

/// The keywords `names` holds, trimmed, blanks and repeats dropped, in their
/// order.
fn keywords_named(names: Vec<String>) -> Vec<String> {
    let mut keywords: Vec<String> = Vec::new();
    for name in names {
        // A name with a space in it would be read back as two keywords.
        for word in name.split_whitespace() {
            if !keywords.iter().any(|kept| kept == word) {
                keywords.push(word.to_string());
            }
        }
    }
    keywords
}

/// The segments `names` call for, in their order; a name that is none is
/// left out and added to `unknown`.
pub(super) fn status_segments_from_names(
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
pub(super) fn unknown_segment_warning(names: &[String]) -> Option<String> {
    let first = names.first()?;
    let known: Vec<&str> = StatusSegmentKind::defaults()
        .into_iter()
        .map(StatusSegmentKind::name)
        .collect();
    Some(format!(
        "settings.kdl: no status-bar segment named {first}; the segments are {}",
        known.join(", ")
    ))
}
