//! The KDL document schema and the readers that turn it into settings.

use super::{ColorOverrides, CursorConfig, SecurityConfig};
use std::collections::HashMap;

use super::theme::ANSI_COLOR_NAMES;
use winter_render::{ControlsSide, CursorShape, ThemeRgb};

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
    pub(crate) keybindings: Option<HashMap<String, HashMap<String, String>>>,
    pub(crate) menu_style: Option<String>,
    pub(crate) scrollback_lines: Option<u64>,
    pub(crate) security: Option<KdlSecurity>,
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
    pub(crate) opacity: Option<f32>,
    pub(crate) theme: Option<String>,
    pub(crate) status_bar: Option<KdlStatusBar>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlCursor {
    pub(crate) blink: Option<bool>,
    pub(crate) block_focus: Option<String>,
    pub(crate) hide_in_inactive: Option<bool>,
    pub(crate) insert: Option<String>,
    pub(crate) normal: Option<String>,
    pub(crate) visual: Option<String>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlSecurity {
    pub(crate) block_max_trust: Option<String>,
    pub(crate) block_remote_assets: Option<bool>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlStatusBar {
    pub(crate) normal_icon: Option<String>,
    pub(crate) insert_icon: Option<String>,
    pub(crate) block_icon: Option<String>,
    pub(crate) show: Option<bool>,
    pub(crate) show_mode: Option<bool>,
}
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) struct KdlColors {
    pub(crate) background: Option<String>,
    pub(crate) foreground: Option<String>,
    pub(crate) cursor_bg: Option<String>,
    pub(crate) cursor_fg: Option<String>,
    pub(crate) scrollbar: Option<String>,
    pub(crate) selection_bg: Option<String>,
    pub(crate) selection_fg: Option<String>,
    pub(crate) split: Option<String>,
    pub(crate) status_bar_border: Option<String>,
    pub(crate) visual_bell: Option<String>,
    pub(crate) window_border: Option<String>,
    // Nested blocks: ansi { red "#c22727" }, brights { red "#d43f30" }
    pub(crate) ansi: Option<HashMap<String, String>>,
    pub(crate) brights: Option<HashMap<String, String>>,
    // Nested block: indexed { "136" "#af8700" }
    pub(crate) indexed: Option<HashMap<String, String>>,
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
/// Parse a KDL boolean-ish string (`"true"`/`"false"`), falling back to `default`.
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
/// A `colors` sub-block (`ansi`/`brights`) of named hex-color nodes (e.g.
/// `ansi { red "#c22727" }`), or `None` when every entry is unset.
pub(super) fn color_named_block_kdl(name: &str, colors: &[Option<ThemeRgb>; 8]) -> Option<String> {
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
pub(super) fn color_overrides_from_kdl(kdl: KdlColors) -> ColorOverrides {
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
/// Apply the `security` block on top of the deny-by-default policy.
///
/// An unparseable tier keeps the default rather than failing the whole config:
/// a typo in `block-max-trust` must never silently *raise* the ceiling, and
/// dropping the entire settings file over one bad word would be worse.
pub(super) fn security_config_from_kdl(kdl: KdlSecurity) -> SecurityConfig {
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
pub(super) fn cursor_config_from_kdl(kdl: KdlCursor) -> CursorConfig {
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
pub(super) fn parse_keys(text: &str) -> HashMap<String, HashMap<String, String>> {
    if text.trim().is_empty() {
        return HashMap::new();
    }
    kdl::de::from_str::<HashMap<String, HashMap<String, String>>>(text).unwrap_or_default()
}
