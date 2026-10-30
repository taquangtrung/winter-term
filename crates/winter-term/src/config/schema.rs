//! The KDL document schema and the readers that turn it into settings.

use std::collections::HashMap;
use std::fmt;

use serde::de::{self, Deserializer, SeqAccess, Visitor};
use winter_core::CursorShape;
pub(super) use winter_render::kdl_string;
use winter_render::{ControlsSide, KdlColors};

use super::diagnostics::config_problems;
use super::CursorConfig;

// ========================================================================
// KDL schema
// ========================================================================

#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlConfig {
    pub(crate) colors: Option<KdlColors>,
    pub(crate) compile: Option<KdlCompile>,
    pub(crate) completion: Option<KdlCompletion>,
    pub(crate) copilot: Option<KdlCopilot>,
    pub(crate) cursor: Option<KdlCursor>,
    pub(crate) dim_inactive: Option<bool>,
    pub(crate) editor_wrap_indent: Option<String>,
    pub(crate) editor_markup_markers: Option<String>,
    pub(crate) font: Option<String>,
    pub(crate) font_size: Option<f32>,
    pub(crate) font_weight: Option<String>,
    pub(crate) font_weight_bold: Option<String>,
    pub(crate) gutter_side: Option<String>,
    pub(crate) lsp: Option<KdlLsp>,
    pub(crate) menu_style: Option<String>,
    pub(crate) scroll_margin: Option<u64>,
    pub(crate) scrollback_lines: Option<u64>,
    pub(crate) shell: Option<String>,
    pub(crate) shell_windows: Option<String>,
    pub(crate) shell_linux: Option<String>,
    pub(crate) shell_macos: Option<String>,
    pub(crate) title_bar_style: Option<String>,
    pub(crate) ligatures: Option<bool>,
    pub(crate) clipboard_read: Option<bool>,
    pub(crate) palette_match_underline: Option<bool>,
    pub(crate) pane_border_width: Option<f32>,
    pub(crate) popup_input_height: Option<f32>,
    pub(crate) paste_on_right_click: Option<bool>,
    pub(crate) prompt_edit_bindings: Option<String>,
    pub(crate) rainbow_parens: Option<bool>,
    pub(crate) restore_session: Option<bool>,
    pub(crate) sentence_highlight: Option<bool>,
    pub(crate) url_underline: Option<bool>,
    pub(crate) window_controls_side: Option<String>,
    pub(crate) window_title_template: Option<String>,
    pub(crate) wrap_indent: Option<bool>,
    pub(crate) wrap_words: Option<bool>,
    pub(crate) opacity: Option<f32>,
    pub(crate) theme: Option<String>,
    #[serde(default, deserialize_with = "present_list")]
    pub(crate) todo_keywords: Option<Vec<String>>,
    pub(crate) status_bar: Option<KdlStatusBar>,
    pub(crate) notifications: Option<KdlNotifications>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlCursor {
    pub(crate) blink: Option<bool>,
    pub(crate) hide_in_inactive: Option<bool>,
    pub(crate) insert: Option<String>,
    pub(crate) normal: Option<String>,
    pub(crate) visual: Option<String>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlStatusBar {
    /// Read as a list whenever the node is there, so a bare `segments` is
    /// an empty list (show none) rather than an absent one (the default).
    #[serde(default, deserialize_with = "present_list")]
    pub(crate) segments: Option<Vec<String>>,
    /// The segments in the order they hold on when the bar is short of
    /// room, the last to go first.
    #[serde(default, deserialize_with = "present_list")]
    pub(crate) priority: Option<Vec<String>>,
    pub(crate) show: Option<bool>,
    pub(crate) show_mode: Option<bool>,
}
/// Reads [`KdlStatusBar::segments`] and [`KdlConfig::todo_keywords`]: see
/// [`present_list`].
struct PresentList;
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlCompile {
    pub(crate) dock: Option<bool>,
    pub(crate) jump_to_error: Option<bool>,
    pub(crate) latex_command: Option<String>,
    pub(crate) latex_on_save: Option<bool>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlCompletion {
    pub(crate) auto: Option<bool>,
    pub(crate) backends: Option<HashMap<String, Names>>,
    pub(crate) delay_ms: Option<u64>,
    pub(crate) min_chars: Option<u64>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlCopilot {
    pub(crate) completions: Option<bool>,
    pub(crate) enable: Option<bool>,
    pub(crate) next_edit: Option<bool>,
    pub(crate) server_path: Option<String>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlLsp {
    pub(crate) auto_start: Option<Names>,
    pub(crate) servers: Option<HashMap<String, Names>>,
}

/// A node's arguments read as names, one or several: what a language's
/// `backends` lists.
pub(super) struct Names(pub(crate) Vec<String>);

impl<'de> serde::Deserialize<'de> for Names {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Names(deserializer.deserialize_any(PresentList)?.unwrap_or_default()))
    }
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlNotifications {
    pub(crate) toasts: Option<bool>,
}
/// A list of names that is there even when it holds none: the KDL reader
/// gives an omitted node as `false`, a bare one as `true`, one name as a
/// string, and several as a sequence.
fn present_list<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    deserializer.deserialize_any(PresentList)
}

// ========================================================================
// PresentList Implementations
// ========================================================================

impl<'de> Visitor<'de> for PresentList {
    type Value = Option<Vec<String>>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a list of names")
    }

    fn visit_bool<E: de::Error>(self, present: bool) -> Result<Self::Value, E> {
        Ok(present.then(Vec::new))
    }

    fn visit_str<E: de::Error>(self, name: &str) -> Result<Self::Value, E> {
        Ok(Some(vec![name.to_string()]))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut names = Vec::new();
        while let Some(name) = seq.next_element()? {
            names.push(name);
        }
        Ok(Some(names))
    }
}

pub(super) fn kdl_bool(value: bool) -> &'static str {
    if value {
        "#true"
    } else {
        "#false"
    }
}
/// Interpret a `window-controls-side` config value: `"left"` (the default)
/// or `"right"`. Unknown values fall back to the default left side.
pub(super) fn controls_side_from_value(value: &str) -> ControlsSide {
    match value.trim().to_ascii_lowercase().as_str() {
        "right" => ControlsSide::Right,
        _ => ControlsSide::Left,
    }
}
/// The canonical config value for a [`ControlsSide`] (round-trips through
/// [`controls_side_from_value`]).
pub(super) fn controls_side_as_value(side: ControlsSide) -> &'static str {
    match side {
        ControlsSide::Left => "left",
        ControlsSide::Right => "right",
    }
}
/// Interpret a `cursor` config value (`"block"`/`"bar"`/`"underline"`).
/// Common synonyms (`"beam"`, `"underscore"`) are accepted; unknown values
/// fall back to `Block` so a typo never produces a missing cursor. Read here,
/// with the rest of the settings, rather than by the terminal model the
/// shape is drawn from.
pub(crate) fn cursor_shape_from_value(value: &str) -> CursorShape {
    match value.trim().to_ascii_lowercase().as_str() {
        "bar" | "beam" | "line" => CursorShape::Bar,
        "underline" | "underscore" => CursorShape::Underline,
        _ => CursorShape::Block,
    }
}

/// The canonical config value for `shape` (round-trips through
/// [`cursor_shape_from_value`]).
pub(crate) fn cursor_shape_as_value(shape: CursorShape) -> &'static str {
    match shape {
        CursorShape::Block => "block",
        CursorShape::Bar => "bar",
        CursorShape::Underline => "underline",
    }
}

/// Apply the `cursor` block on top of `defaults`; any unset entry keeps its
/// value there. Unknown shape strings fall back to `Block` via
/// [`cursor_shape_from_value`].
pub(super) fn cursor_config_from_kdl(kdl: KdlCursor, defaults: &CursorConfig) -> CursorConfig {
    CursorConfig {
        blink: kdl.blink.unwrap_or(defaults.blink),
        hide_in_inactive: kdl.hide_in_inactive.unwrap_or(defaults.hide_in_inactive),
        insert: kdl
            .insert
            .as_deref()
            .map(cursor_shape_from_value)
            .unwrap_or(defaults.insert),
        normal: kdl
            .normal
            .as_deref()
            .map(cursor_shape_from_value)
            .unwrap_or(defaults.normal),
        visual: kdl
            .visual
            .as_deref()
            .map(cursor_shape_from_value)
            .unwrap_or(defaults.visual),
    }
}
/// Parse a standalone `keybindings.kdl` into the mode -> (key -> action) map. An
/// empty file is an empty map (callers keep their defaults); one that does not
/// parse says why, so the user learns their bindings were not applied.
pub(super) fn parse_keys(text: &str) -> Result<HashMap<String, HashMap<String, String>>, String> {
    if text.trim().is_empty() {
        return Ok(HashMap::new());
    }
    kdl::de::from_str::<HashMap<String, HashMap<String, String>>>(text).map_err(|e| {
        let problems = config_problems(&e, text);
        match problems.first() {
            Some(first) => format!(
                "{} problem(s); line {}: {}",
                problems.len(),
                first.line,
                first.message
            ),
            None => e.to_string(),
        }
    })
}
