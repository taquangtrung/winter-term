//! Extensions' settings in `settings.kdl`: a block named for each
//! extension's id, as `copilot { enable #true }`, holding the settings it
//! declared (see [`Registry::setting`]).
//!
//! A top-level block that is none of Winter's own settings is taken out of
//! the text before Winter's are read, spaces left where it stood so every
//! line keeps its number, and read as an extension's. One no extension
//! installed claims is kept as written, and said once, rather than refusing
//! the whole file: an extension removed leaves its block harmless.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use kdl::{KdlDocument, KdlNode, KdlValue};
use serde::de::{self, Deserialize, Visitor};
use winter_ext_api::{Registry, SettingValue, Settings};

use super::schema::{kdl_bool, kdl_string, KdlConfig};

// ========================================================================
// Constants
// ========================================================================

/// The names of Winter's own top-level settings.
static OWN_SETTINGS: LazyLock<&'static [&'static str]> = LazyLock::new(own_settings);

// ========================================================================
// Data Structures
// ========================================================================

/// What `settings.kdl` sets of the extensions' settings: each block, by the
/// extension it is named for, holding the values it gives, by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ExtensionSettings {
    blocks: BTreeMap<String, BTreeMap<String, SettingValue>>,
}

/// The text of a settings file with the extensions' blocks taken out, and
/// those blocks.
pub(super) struct Split {
    pub(super) blocks: Vec<KdlNode>,
    pub(super) own: String,
}

/// A deserializer that only learns the names of the fields a struct has.
struct FieldNames(&'static [&'static str]);

/// Why [`FieldNames`] stops: it has learned them, or found no struct.
#[derive(Debug)]
struct Learned;

// ========================================================================
// ExtensionSettings
// ========================================================================

impl ExtensionSettings {
    /// The settings of `extension`, as `registry` declares them: the value
    /// `settings.kdl` gives each, its default where it gives none.
    pub(crate) fn of(&self, registry: &Registry, extension: &str) -> Settings {
        let set = self.blocks.get(extension);
        let values = registry
            .settings()
            .iter()
            .filter(|setting| setting.extension == extension)
            .map(|setting| {
                let value = set
                    .and_then(|set| set.get(&setting.name))
                    .unwrap_or(&setting.default);
                (setting.name.clone(), value.clone())
            })
            .collect();
        Settings::new(values)
    }

    /// Set `extension`'s setting `name` to `value`.
    pub(crate) fn set(&mut self, extension: &str, name: &str, value: SettingValue) {
        let values = self.blocks.entry(extension.to_string()).or_default();
        values.insert(name.to_string(), value);
    }

    /// Take what `other` sets over what these do, setting by setting.
    pub(super) fn merge(&mut self, other: Self) {
        for (extension, values) in other.blocks {
            self.blocks.entry(extension).or_default().extend(values);
        }
    }

    /// The blocks as `settings.kdl` text, which reads back as the same.
    pub(super) fn to_kdl(&self) -> String {
        let mut out = String::new();
        for (extension, values) in &self.blocks {
            out.push_str(&format!("{extension} {{\n"));
            for (name, value) in values {
                out.push_str(&format!("    {name} {}\n", value_kdl(value)));
            }
            out.push_str("}\n");
        }
        out
    }
}

// ========================================================================
// FieldNames
// ========================================================================

impl std::fmt::Display for Learned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("field names learned")
    }
}

impl std::error::Error for Learned {}

impl de::Error for Learned {
    fn custom<T: std::fmt::Display>(_message: T) -> Self {
        Self
    }
}

impl<'de> de::Deserializer<'de> for &mut FieldNames {
    type Error = Learned;

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Learned> {
        Err(Learned)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Learned> {
        self.0 = fields;
        Err(Learned)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map enum identifier ignored_any
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The names of [`KdlConfig`]'s fields, as `settings.kdl` writes them.
fn own_settings() -> &'static [&'static str] {
    let mut names = FieldNames(&[]);
    let _ = KdlConfig::deserialize(&mut names);
    names.0
}

/// `text` with every top-level block that is none of Winter's own settings
/// taken out, spaces in its place, and those blocks. Text that is not KDL
/// is left whole, for Winter's own reading to say what is wrong with it.
pub(super) fn split(text: &str) -> Split {
    let Ok(document) = KdlDocument::parse(text) else {
        return Split {
            blocks: Vec::new(),
            own: text.to_string(),
        };
    };
    let mut own = text.to_string();
    let mut blocks = Vec::new();
    for node in document.nodes() {
        if OWN_SETTINGS.contains(&node.name().value()) {
            continue;
        }
        let span = node.span();
        let range = span.offset()..span.offset() + span.len();
        let blanked: String = text[range.clone()]
            .chars()
            .map(|c| if c == '\n' { '\n' } else { ' ' })
            .collect();
        own.replace_range(range, &blanked);
        blocks.push(node.clone());
    }
    Split { blocks, own }
}

/// Read `blocks` as the settings of the extensions `registry` declares:
/// what they set, and what to say of what none declares.
pub(super) fn read(blocks: &[KdlNode], registry: &Registry) -> (ExtensionSettings, Option<String>) {
    let mut read = ExtensionSettings::default();
    let mut slips = Vec::new();
    for block in blocks {
        let extension = block.name().value();
        let declared: Vec<_> = registry
            .settings()
            .iter()
            .filter(|setting| setting.extension == extension)
            .collect();
        if declared.is_empty() {
            slips.push(format!(
                "`{extension}` is no setting, nor an installed extension's"
            ));
            continue;
        }
        let values = read.blocks.entry(extension.to_string()).or_default();
        for node in block.children().map(KdlDocument::nodes).unwrap_or_default() {
            let name = node.name().value();
            let Some(setting) = declared.iter().find(|setting| setting.name == name) else {
                slips.push(format!("{extension} has no setting `{name}`"));
                continue;
            };
            let value = node.entries().first().map(|entry| entry.value());
            match value.and_then(|value| value_as(value, &setting.default)) {
                Some(value) => {
                    values.insert(name.to_string(), value);
                }
                None => slips.push(format!(
                    "{extension}'s {name} takes {}",
                    kind_said(&setting.default)
                )),
            }
        }
    }
    let said = slips.first().map(|first| match slips.len() {
        1 => format!("settings.kdl: {first}"),
        n => format!("settings.kdl: {first} (and {} more)", n - 1),
    });
    (read, said)
}

/// `value` as the kind of value `default` is, if it is one.
fn value_as(value: &KdlValue, default: &SettingValue) -> Option<SettingValue> {
    match (value, default) {
        (KdlValue::Bool(on), SettingValue::Switch(_)) => Some(SettingValue::Switch(*on)),
        (KdlValue::Integer(number), SettingValue::Number(_)) => {
            i64::try_from(*number).ok().map(SettingValue::Number)
        }
        (KdlValue::String(text), SettingValue::Text(_)) => Some(SettingValue::Text(text.clone())),
        _ => None,
    }
}

/// What kind of value `default` is, as a slip says.
fn kind_said(default: &SettingValue) -> &'static str {
    match default {
        SettingValue::Number(_) => "a whole number",
        SettingValue::Switch(_) => "#true or #false",
        SettingValue::Text(_) => "text in quotes",
        _ => "another kind of value",
    }
}

/// `value` as `settings.kdl` writes it.
fn value_kdl(value: &SettingValue) -> String {
    match value {
        SettingValue::Number(number) => number.to_string(),
        SettingValue::Switch(on) => kdl_bool(*on).to_string(),
        SettingValue::Text(text) => kdl_string(text),
        _ => String::new(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use winter_ext_api::Extension;

    /// An extension declaring a switch, a number, and some text.
    struct Declares;

    impl Extension for Declares {
        fn id(&self) -> &'static str {
            "pilot"
        }

        fn activate(&self, registry: &mut Registry) {
            registry.setting("enable", SettingValue::Switch(true), "On.");
            registry.setting("delay", SettingValue::Number(75), "Wait.");
            registry.setting("path", SettingValue::Text(String::new()), "Where.");
        }
    }

    fn registry() -> Registry {
        Registry::activate_all([&Declares as &dyn Extension])
    }

    #[test]
    fn test_winter_s_own_settings_are_known_by_name() {
        assert!(OWN_SETTINGS.contains(&"font-size"));
        assert!(OWN_SETTINGS.contains(&"status-bar"));
        assert!(!OWN_SETTINGS.contains(&"pilot"));
    }

    #[test]
    fn test_an_extension_s_block_is_taken_out_with_every_line_kept() {
        let text = "font-size 13\npilot {\n    enable #false\n}\ntheme \"dark\"\n";
        let split = split(text);
        assert_eq!(split.blocks.len(), 1);
        assert_eq!(split.own.lines().count(), text.lines().count());
        assert!(split.own.contains("font-size 13") && split.own.contains("theme"));
        assert!(!split.own.contains("pilot") && !split.own.contains("enable"));
    }

    #[test]
    fn test_a_block_s_values_override_the_defaults_and_slips_are_said() {
        let text = "pilot {\n    enable #false\n    delay \"soon\"\n    colour 3\n}\n";
        let (read, said) = read(&split(text).blocks, &registry());
        let settings = read.of(&registry(), "pilot");
        assert_eq!(settings.switch("enable"), Some(false), "set");
        assert_eq!(
            settings.number("delay"),
            Some(75),
            "the wrong kind keeps the default"
        );
        assert_eq!(settings.text("path"), Some(""), "left out: the default");
        assert_eq!(
            said.as_deref(),
            Some("settings.kdl: pilot's delay takes a whole number (and 1 more)")
        );
    }

    #[test]
    fn test_a_block_no_extension_claims_is_said_and_the_rest_read() {
        let (read, said) = read(&split("gone {\n    x 1\n}\n").blocks, &registry());
        assert_eq!(read, ExtensionSettings::default());
        assert_eq!(
            said.as_deref(),
            Some("settings.kdl: `gone` is no setting, nor an installed extension's")
        );
    }

    #[test]
    fn test_the_blocks_written_read_back_as_the_same() {
        let text = "pilot {\n    enable #false\n    path \"/opt/a b\"\n}\n";
        let (first, _) = read(&split(text).blocks, &registry());
        let (again, said) = read(&split(&first.to_kdl()).blocks, &registry());
        assert_eq!(again, first);
        assert_eq!(said, None);
    }
}
