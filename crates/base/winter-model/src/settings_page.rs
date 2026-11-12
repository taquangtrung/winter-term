//! The Settings page's model: its rows, each editing one setting through a
//! control, and where edits are kept. How the page is drawn and edited is
//! the app's; each row's value is written as `settings.kdl` writes it, for
//! the app to put in force through its `apply_setting`.

use std::collections::BTreeSet;

// ========================================================================
// Data Structures
// ========================================================================

/// The editable control behind one settings row.
#[derive(Clone, Debug)]
pub enum Control {
    /// A field cycling through named options.
    Choice(ChoiceControl),
    /// A numeric field with a range and step.
    Number(NumberControl),
    /// A free-text field.
    Text(TextControl),
    /// A field that is either on or off.
    Toggle(ToggleControl),
}

/// A cycling enum control (theme, menu style): the chosen `index` into `options`.
#[derive(Clone, Debug)]
pub struct ChoiceControl {
    /// Index of the currently selected option.
    pub index: usize,
    /// Every option this field can take.
    pub options: Vec<ChoiceOption>,
}

/// One option of a [`ChoiceControl`]: the `label` shown to the user and the
/// `value` written to the config. They differ for e.g. "Modern" -> "modern".
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChoiceOption {
    /// What the option shows in the UI.
    pub label: String,
    /// The value written to the config file.
    pub value: String,
}

/// A numeric control adjusted in fixed `step`s and clamped to `[min, max]`.
/// `decimals` controls how the value is formatted for both display and config.
#[derive(Clone, Debug)]
pub struct NumberControl {
    /// How many decimal places to display.
    pub decimals: usize,
    /// Largest accepted value.
    pub max: f32,
    /// Smallest accepted value.
    pub min: f32,
    /// How much one adjustment moves the value.
    pub step: f32,
    /// The current value.
    pub value: f32,
}

/// One row of the settings page: a config `key`, its human `label`, and the
/// `control` that edits it. `section`, when set, starts a new titled group above
/// this row; `note` is an optional dim one-line description (VSCode-style).
#[derive(Clone, Debug)]
pub struct SettingsField {
    /// The control this field renders as.
    pub control: Control,
    /// The config key this field writes.
    pub key: String,
    /// The field's label in the UI.
    pub label: String,
    /// Extra explanation shown alongside the field.
    pub note: Option<String>,
    /// Heading this field is grouped under.
    pub section: Option<String>,
}

/// Where an edit on the settings page is kept, as VS Code's User and
/// Workspace tabs choose.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Scope {
    /// In `settings.kdl`, for every window and workspace.
    #[default]
    User,
    /// In the window's workspace alone, over `settings.kdl`.
    Workspace,
}

/// The settings page: an ordered list of `fields`, and where edits are kept.
#[derive(Clone, Debug)]
pub struct SettingsPage {
    /// Every field on the page, in display order.
    pub fields: Vec<SettingsField>,
    /// The keys of the rows the window's workspace sets over `settings.kdl`.
    pub overridden: BTreeSet<String>,
    /// Where an edit is kept.
    pub scope: Scope,
    /// The window's workspace, by name; `None` when it is in none, and only
    /// the User scope is offered.
    pub workspace: Option<String>,
}

/// A free-text control (font family), and what kind of text it holds.
#[derive(Clone, Debug)]
pub struct TextControl {
    /// What the text is, which says how it is drawn.
    pub kind: TextKind,
    /// The current text.
    pub value: String,
}

/// What a text row holds.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum TextKind {
    /// A color, `#rrggbb` or `#rrggbbaa`, drawn beside a swatch of it.
    Color,
    /// Names separated by spaces, drawn one by one, the names it may take
    /// that it does not yet hold offered to add.
    Names(Vec<String>),
    /// Text as typed.
    #[default]
    Plain,
}

/// An on/off control (status-bar visibility flags).
#[derive(Clone, Debug)]
pub struct ToggleControl {
    /// Whether the setting is enabled.
    pub on: bool,
}

// ========================================================================
// Control
// ========================================================================

impl Control {
    /// The config value this control serializes to, as `apply_setting` expects it.
    pub fn value(&self) -> String {
        match self {
            Self::Choice(c) => c
                .options
                .get(c.index)
                .map(|o| o.value.clone())
                .unwrap_or_default(),
            Self::Number(n) => format!("{:.*}", n.decimals, n.value),
            Self::Text(t) => t.value.clone(),
            Self::Toggle(t) => if t.on { "true" } else { "false" }.to_string(),
        }
    }
}

// ========================================================================
// SettingsField
// ========================================================================

impl SettingsField {
    /// A field cycling through named options, starting on the one at `index`.
    pub fn choice(key: &str, label: &str, options: Vec<ChoiceOption>, index: usize) -> Self {
        Self::new(
            key,
            label,
            Control::Choice(ChoiceControl { index, options }),
        )
    }

    /// A numeric field clamped to `min..=max` and stepped by `step`.
    pub fn number(
        key: &str,
        label: &str,
        value: f32,
        min: f32,
        max: f32,
        step: f32,
        decimals: usize,
    ) -> Self {
        Self::new(
            key,
            label,
            Control::Number(NumberControl {
                decimals,
                max,
                min,
                step,
                value,
            }),
        )
    }

    /// A free-text field.
    pub fn text(key: &str, label: &str, value: String) -> Self {
        Self::text_of(key, label, value, TextKind::Plain)
    }

    /// A color field, `#rrggbb` or `#rrggbbaa`, empty for the theme's own.
    pub fn color(key: &str, label: &str, value: String) -> Self {
        Self::text_of(key, label, value, TextKind::Color)
    }

    /// A field of names, `names` in order, offering the `choices` it may
    /// take.
    pub fn names(key: &str, label: &str, names: &[String], choices: Vec<String>) -> Self {
        Self::text_of(key, label, names.join(" "), TextKind::Names(choices))
    }

    fn text_of(key: &str, label: &str, value: String, kind: TextKind) -> Self {
        Self::new(key, label, Control::Text(TextControl { kind, value }))
    }

    /// A field toggling between on and off.
    pub fn toggle(key: &str, label: &str, on: bool) -> Self {
        Self::new(key, label, Control::Toggle(ToggleControl { on }))
    }

    /// Start a new titled section above this row.
    pub fn in_section(mut self, section: &str) -> Self {
        self.section = Some(section.to_string());
        self
    }

    /// Attach a dim one-line description shown under the label.
    pub fn with_note(mut self, note: &str) -> Self {
        self.note = Some(note.to_string());
        self
    }

    fn new(key: &str, label: &str, control: Control) -> Self {
        Self {
            control,
            key: key.to_string(),
            label: label.to_string(),
            note: None,
            section: None,
        }
    }
}

// ========================================================================
// SettingsPage
// ========================================================================

impl SettingsPage {
    /// A page over these fields, its edits kept in the User scope.
    pub fn new(fields: Vec<SettingsField>) -> Self {
        Self {
            fields,
            overridden: BTreeSet::new(),
            scope: Scope::User,
            workspace: None,
        }
    }

    /// Offer the Workspace scope, for the window's workspace `name` setting
    /// the rows `overridden` over `settings.kdl`.
    pub fn with_workspace(mut self, name: String, overridden: BTreeSet<String>) -> Self {
        self.workspace = Some(name);
        self.overridden = overridden;
        self
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_each_control_writes_its_value_as_settings_kdl_does() {
        let choice = SettingsField::choice(
            "theme",
            "Theme",
            vec![
                ChoiceOption {
                    label: "Dark".into(),
                    value: "dark".into(),
                },
                ChoiceOption {
                    label: "Light".into(),
                    value: "light".into(),
                },
            ],
            1,
        );
        assert_eq!(choice.control.value(), "light", "the value, not the label");
        let number = SettingsField::number("opacity", "Opacity", 0.9, 0.1, 1.0, 0.05, 2);
        assert_eq!(number.control.value(), "0.90", "at its decimals");
        let toggle = SettingsField::toggle("status.enabled", "Show status bar", true);
        assert_eq!(toggle.control.value(), "true");
    }
}
