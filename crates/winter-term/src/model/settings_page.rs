//! Native settings page: a keyboard-driven, text-mode overlay that replaces the
//! cross-platform-fragile WebView form. Each row edits one config setting; the
//! mutating methods return the changed `(key, value)` so the app can route it
//! through the same `apply_setting` path the WebView used. Rendering lives in the
//! app's `render` module, which paints this model as a terminal grid, so it works
//! on every platform the GPU renderer does (the WebView did not on Linux).

use std::collections::BTreeSet;

use crate::model::input::Key;
use crate::model::textedit::field::TextField;

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

/// The open settings page: an ordered list of `fields` and the `selected` row.
#[derive(Clone, Debug)]
pub struct SettingsPage {
    /// The selected text row's value as a one-line field, for its Emacs keys:
    /// it keeps the caret and the kill ring between keys, and follows the
    /// row's value wherever something else sets it.
    editor: TextField,
    /// Every field on the page, in display order.
    pub fields: Vec<SettingsField>,
    /// The keys of the rows the window's workspace sets over `settings.kdl`.
    pub overridden: BTreeSet<String>,
    /// Where an edit is kept.
    pub scope: Scope,
    /// Index of the field the cursor is on.
    pub selected: usize,
    /// The window's workspace, by name; `None` when it is in none, and only
    /// the User scope is offered.
    pub workspace: Option<String>,
}

/// A free-text control (font family).
#[derive(Clone, Debug)]
pub struct TextControl {
    /// The current text.
    pub value: String,
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
        Self::new(key, label, Control::Text(TextControl { value }))
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
    /// A page over these fields, with the first one selected.
    pub fn new(fields: Vec<SettingsField>) -> Self {
        Self {
            editor: TextField::line(""),
            fields,
            overridden: BTreeSet::new(),
            scope: Scope::User,
            selected: 0,
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

    /// Go from one scope to the other. Without a workspace there is only
    /// the User scope, and nothing changes; returns whether it did.
    pub fn toggle_scope(&mut self) -> bool {
        if self.workspace.is_none() {
            return false;
        }
        self.scope = match self.scope {
            Scope::User => Scope::Workspace,
            Scope::Workspace => Scope::User,
        };
        true
    }

    /// The key of the selected row.
    pub fn selected_key(&self) -> Option<&str> {
        self.fields
            .get(self.selected)
            .map(|field| field.key.as_str())
    }

    /// Edit the selected text row at its caret with `key`, as a one-line
    /// field takes it. Returns the changed `(key, value)`, or `None` if the
    /// row is not a text field or the key changed nothing.
    pub fn edit_text(&mut self, key: &Key) -> Option<(String, String)> {
        self.edit_with(|editor| {
            editor.on_key(key, 1);
        })
    }

    /// Paste `text`'s first line into the selected text row at its caret.
    /// Returns the changed `(key, value)`, as [`Self::edit_text`] does.
    pub fn paste_text(&mut self, text: &str) -> Option<(String, String)> {
        self.edit_with(|editor| editor.paste(text))
    }

    /// Run `edit` on the selected text row's field, and take what it left as
    /// the row's value.
    fn edit_with(&mut self, edit: impl FnOnce(&mut TextField)) -> Option<(String, String)> {
        let field = self.fields.get_mut(self.selected)?;
        let Control::Text(text) = &mut field.control else {
            return None;
        };
        if self.editor.text() != text.value {
            self.editor.set_text(&text.value);
        }
        edit(&mut self.editor);
        if self.editor.text() == text.value {
            return None;
        }
        text.value = self.editor.text().to_string();
        Some((field.key.clone(), field.control.value()))
    }

    /// Where the caret sits in the selected text row, in characters: where
    /// the field's keys left it, or the end when something else set the
    /// value. `None` when the selected row is not a text field.
    pub fn text_caret(&self) -> Option<usize> {
        let Control::Text(text) = &self.fields.get(self.selected)?.control else {
            return None;
        };
        Some(match self.editor.text() == text.value {
            true => self.editor.col(),
            false => text.value.chars().count(),
        })
    }

    /// Move the selection to the previous field, stopping at the first.
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move the selection to the next field, stopping at the last.
    pub fn move_down(&mut self) {
        if self.selected + 1 < self.fields.len() {
            self.selected += 1;
        }
    }

    /// Whether the selected row holds a text field, so the caller can route a
    /// space key to typing rather than to value adjustment.
    pub fn selected_is_text(&self) -> bool {
        matches!(
            self.fields.get(self.selected).map(|f| &f.control),
            Some(Control::Text(_))
        )
    }

    /// Step the selected control: cycle a choice, increment/decrement a number,
    /// or flip a toggle (either direction). Returns the changed `(key, value)` to
    /// persist, or `None` if nothing changed (e.g. a text field, or a number at
    /// its bound).
    pub fn adjust(&mut self, forward: bool) -> Option<(String, String)> {
        let field = self.fields.get_mut(self.selected)?;
        match &mut field.control {
            Control::Choice(c) => {
                let n = c.options.len();
                if n <= 1 {
                    return None;
                }
                c.index = if forward {
                    (c.index + 1) % n
                } else {
                    (c.index + n - 1) % n
                };
            }
            Control::Number(num) => {
                let next = if forward {
                    num.value + num.step
                } else {
                    num.value - num.step
                };
                let clamped = next.clamp(num.min, num.max);
                if (clamped - num.value).abs() < f32::EPSILON {
                    return None;
                }
                num.value = clamped;
            }
            Control::Toggle(t) => t.on = !t.on,
            Control::Text(_) => return None,
        }
        Some((field.key.clone(), field.control.value()))
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_workspace_scope_is_offered_only_with_a_workspace() {
        let fields = || vec![SettingsField::toggle("ligatures", "Ligatures", true)];
        let mut alone = SettingsPage::new(fields());
        assert!(!alone.toggle_scope());
        assert_eq!(alone.scope, Scope::User);

        let mut page = SettingsPage::new(fields()).with_workspace("w".into(), BTreeSet::new());
        assert!(page.toggle_scope());
        assert_eq!(page.scope, Scope::Workspace);
        assert!(page.toggle_scope());
        assert_eq!(page.scope, Scope::User, "and back");
    }

    fn page() -> SettingsPage {
        SettingsPage::new(vec![
            SettingsField::choice(
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
                0,
            ),
            SettingsField::toggle("status.enabled", "Show status bar", true),
            SettingsField::text("font_family", "Font family", String::new()),
            SettingsField::number("opacity", "Opacity", 0.9, 0.1, 1.0, 0.05, 2),
        ])
    }

    #[test]
    fn test_choice_cycles_and_wraps() {
        let mut p = page();
        assert_eq!(p.adjust(true), Some(("theme".into(), "light".into())));
        // Wraps back to the first option.
        assert_eq!(p.adjust(true), Some(("theme".into(), "dark".into())));
        // Backward from the first wraps to the last.
        assert_eq!(p.adjust(false), Some(("theme".into(), "light".into())));
    }

    #[test]
    fn test_toggle_flips_both_directions() {
        let mut p = page();
        p.selected = 1;
        assert_eq!(
            p.adjust(true),
            Some(("status.enabled".into(), "false".into()))
        );
        assert_eq!(
            p.adjust(false),
            Some(("status.enabled".into(), "true".into()))
        );
    }

    #[test]
    fn test_number_clamps_at_bounds() {
        let mut p = page();
        p.selected = 3;
        assert_eq!(p.adjust(true), Some(("opacity".into(), "0.95".into())));
        assert_eq!(p.adjust(true), Some(("opacity".into(), "1.00".into())));
        // Already at max: no further change.
        assert_eq!(p.adjust(true), None);
    }

    #[test]
    fn test_text_only_edits_text_fields() {
        let typed = |c| Key::plain(crate::model::input::KeyCode::Char(c));
        let mut p = page();
        // A toggle row ignores typed characters.
        p.selected = 1;
        assert_eq!(p.edit_text(&typed('x')), None);
        // The text row accepts them.
        p.selected = 2;
        assert_eq!(
            p.edit_text(&typed('M')),
            Some(("font_family".into(), "M".into()))
        );
        assert_eq!(
            p.edit_text(&typed('S')),
            Some(("font_family".into(), "MS".into()))
        );
        assert!(p.selected_is_text());
    }

    #[test]
    fn test_a_text_row_is_edited_at_its_caret_with_the_emacs_keys() {
        let key = |c, ctrl| Key {
            ctrl,
            ..Key::plain(crate::model::input::KeyCode::Char(c))
        };
        let mut p = page();
        p.selected = 2;
        p.paste_text("Mono");
        p.edit_text(&key('a', true));
        assert_eq!(p.text_caret(), Some(0));
        assert_eq!(
            p.edit_text(&key('X', false)),
            Some(("font_family".into(), "XMono".into()))
        );
        assert_eq!(p.text_caret(), Some(1));
        assert_eq!(
            p.edit_text(&key('k', true)),
            Some(("font_family".into(), "X".into()))
        );
        p.selected = 1;
        assert_eq!(p.text_caret(), None, "a toggle row has no caret");
    }

    #[test]
    fn test_navigation_clamps_to_field_range() {
        let mut p = page();
        p.move_up();
        assert_eq!(p.selected, 0);
        for _ in 0..10 {
            p.move_down();
        }
        assert_eq!(p.selected, p.fields.len() - 1);
    }
}
