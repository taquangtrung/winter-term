//! The extensions' settings on the settings page: a row for each one an
//! extension declared (see `winter_ext_api::Registry::setting`), in a section
//! named for its extension, keyed `extension.<id>.<name>`.

use winter_ext_api::{Setting, SettingValue};
use winter_model::settings_page::SettingsField;

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// What an extension setting's row key starts with, before its
/// extension's id and its name.
const KEY_PREFIX: &str = "extension.";

/// The range and step a number setting is set over.
const NUMBER_RANGE: (f32, f32) = (0.0, 1_000_000.0);
const NUMBER_STEP: f32 = 1.0;

// ========================================================================
// App: extensions' settings
// ========================================================================

impl App {
    /// A row for every setting an extension declared, each extension's
    /// first headed by its section.
    pub(super) fn extension_setting_fields(&self) -> Vec<SettingsField> {
        let registry = crate::extensions::registry();
        let mut fields = Vec::new();
        let mut last = None;
        for setting in registry.settings() {
            let now = self
                .config
                .extension_settings
                .of(registry, setting.extension);
            let value = now.get(&setting.name).unwrap_or(&setting.default);
            let field = field_of(setting, value).with_note(&setting.doc);
            fields.push(match last == Some(setting.extension) {
                true => field,
                false => field.in_section(&section_of(setting.extension)),
            });
            last = Some(setting.extension);
        }
        fields
    }

    /// Take the settings page's `value` for the row `key`, where it is an
    /// extension setting's. Says whether it was one.
    pub(super) fn apply_extension_setting(&mut self, key: &str, value: &str) -> bool {
        let Some((extension, name)) = key
            .strip_prefix(KEY_PREFIX)
            .and_then(|rest| rest.split_once('.'))
        else {
            return false;
        };
        let registry = crate::extensions::registry();
        let Some(setting) = registry
            .settings()
            .iter()
            .find(|setting| setting.extension == extension && setting.name == name)
        else {
            return false;
        };
        let Some(value) = value_from(&setting.default, value) else {
            return false;
        };
        self.config.extension_settings.set(extension, name, value);
        true
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The row for `setting`, showing `value`.
fn field_of(setting: &Setting, value: &SettingValue) -> SettingsField {
    let key = format!("{KEY_PREFIX}{}.{}", setting.extension, setting.name);
    let label = label_of(&setting.name);
    match value {
        SettingValue::Number(number) => {
            let (min, max) = NUMBER_RANGE;
            SettingsField::number(&key, &label, *number as f32, min, max, NUMBER_STEP, 0)
        }
        SettingValue::Switch(on) => SettingsField::toggle(&key, &label, *on),
        SettingValue::Text(text) => SettingsField::text(&key, &label, text.clone()),
        _ => SettingsField::text(&key, &label, String::new()),
    }
}

/// The value the page's `value` is, as the kind `default` is.
fn value_from(default: &SettingValue, value: &str) -> Option<SettingValue> {
    match default {
        SettingValue::Number(_) => value
            .trim()
            .parse::<f64>()
            .ok()
            .map(|number| SettingValue::Number(number.round() as i64)),
        SettingValue::Switch(_) => Some(SettingValue::Switch(value == "true")),
        SettingValue::Text(_) => Some(SettingValue::Text(value.trim().to_string())),
        _ => None,
    }
}

/// What a setting's row says it is: its name, its first letter capital and
/// its dashes spaces, `next-edit` as `Next edit`.
fn label_of(name: &str) -> String {
    let spaced = name.replace('-', " ");
    let mut letters = spaced.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

/// The section an extension's settings are under: its id, capitalized.
fn section_of(extension: &str) -> String {
    label_of(extension)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::extensions::tests::LOUD;

    #[test]
    fn test_an_extension_s_setting_has_a_row_that_writes_it() {
        let mut app = App::new();
        let fields = app.extension_setting_fields();
        let key = format!("{KEY_PREFIX}probe.{LOUD}");
        let row = fields.iter().find(|field| field.key == key).expect("a row");
        assert_eq!(row.section.as_deref(), Some("Probe"));
        assert!(app.apply_extension_setting(&key, "true"));
        let registry = crate::extensions::registry();
        let now = app.config.extension_settings.of(registry, "probe");
        assert_eq!(now.switch(LOUD), Some(true));
        assert!(!app.apply_extension_setting("extension.probe.none", "true"));
    }

    #[test]
    fn test_a_setting_s_label_reads_as_words() {
        assert_eq!(label_of("next-edit"), "Next edit");
        assert_eq!(label_of("enable"), "Enable");
    }
}
