//! The KDL document schema and the readers that turn it into settings.

use std::collections::HashMap;
use std::fmt;

use serde::de::{self, Deserializer, SeqAccess, Visitor};
use winter_render::{ControlsSide, CursorShape};

use super::colors::KdlColors;
use super::diagnostics::config_problems;
use super::CursorConfig;

// ========================================================================
// KDL schema
// ========================================================================

#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlConfig {
    pub(crate) colors: Option<KdlColors>,
    pub(crate) cursor: Option<KdlCursor>,
    pub(crate) dim_inactive: Option<bool>,
    pub(crate) font: Option<String>,
    pub(crate) font_size: Option<f32>,
    pub(crate) font_weight: Option<String>,
    pub(crate) font_weight_bold: Option<String>,
    pub(crate) gutter_side: Option<String>,
    /// The retired icon style, read and ignored: configs written while it was
    /// a setting still name it, and an unknown key fails the whole file.
    #[serde(rename = "icons")]
    _icons: Option<String>,
    pub(crate) keybindings: Option<HashMap<String, HashMap<String, String>>>,
    pub(crate) menu_style: Option<String>,
    pub(crate) scrollback_lines: Option<u64>,
    /// The retired `security` block, read and ignored: blocks no longer run
    /// in a web engine, so there is no scripting or network to grant, and
    /// configs written before still carry it.
    #[serde(rename = "security")]
    _security: Option<KdlSecurity>,
    pub(crate) shell: Option<String>,
    pub(crate) shell_windows: Option<String>,
    pub(crate) shell_linux: Option<String>,
    pub(crate) shell_macos: Option<String>,
    pub(crate) title_bar_style: Option<String>,
    pub(crate) ligatures: Option<bool>,
    pub(crate) clipboard_read: Option<bool>,
    pub(crate) palette_match_underline: Option<bool>,
    pub(crate) pane_border_width: Option<f32>,
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
    pub(crate) status_bar: Option<KdlStatusBar>,
    pub(crate) notifications: Option<KdlNotifications>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlCursor {
    pub(crate) blink: Option<bool>,
    /// The retired block-focus cursor shape, read and ignored: there is no
    /// longer a mode for it, and configs written before still name it.
    #[serde(rename = "block-focus")]
    _block_focus: Option<String>,
    pub(crate) hide_in_inactive: Option<bool>,
    pub(crate) insert: Option<String>,
    pub(crate) normal: Option<String>,
    pub(crate) visual: Option<String>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlSecurity {
    #[serde(rename = "block-max-trust")]
    _block_max_trust: Option<String>,
    #[serde(rename = "block-remote-assets")]
    _block_remote_assets: Option<bool>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlStatusBar {
    /// Read as a list whenever the node is there, so a bare `segments` is
    /// an empty list (show none) rather than an absent one (the default).
    #[serde(default, deserialize_with = "present_list")]
    pub(crate) segments: Option<Vec<String>>,
    pub(crate) show: Option<bool>,
    pub(crate) show_mode: Option<bool>,
}
/// Reads [`KdlStatusBar::segments`]: see [`present_list`].
struct PresentList;
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlNotifications {
    pub(crate) toasts: Option<bool>,
}
/// A `themes/<name>.kdl` file: an optional `base` preset (`dark`/`light`) plus a
/// `colors` block layered over it, reusing the same color schema as the main
/// config.
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlThemeFile {
    pub(crate) base: Option<String>,
    pub(crate) colors: Option<KdlColors>,
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

/// Quote `s` as a KDL string argument, escaping backslashes and double quotes.
pub(super) fn kdl_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
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
/// Apply the `cursor` block on top of the default per-mode shapes; any unset
/// entry keeps its default. Unknown shape strings fall back to `Block` via
/// [`CursorShape::from_value`].
pub(super) fn cursor_config_from_kdl(kdl: KdlCursor) -> CursorConfig {
    let defaults = CursorConfig::default();
    CursorConfig {
        blink: kdl.blink.unwrap_or(defaults.blink),
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
