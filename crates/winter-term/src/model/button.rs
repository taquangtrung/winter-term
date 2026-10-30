//! Modular button groups: button state, focus navigation, and keyboard dispatch.

use winter_render::ButtonView;

use crate::model::input::{Key, KeyCode, Mods};

// ========================================================================
// Data Structures
// ========================================================================

/// A button in a button group.
#[derive(Clone, Debug, PartialEq)]
pub struct Button<T> {
    /// Button label text (e.g. "Yes", "No", "Cancel").
    pub label: String,
    /// Whether this button represents the primary/default action.
    pub primary: bool,
    /// Direct shortcut character (e.g. 'y' or 'n').
    pub shortcut_char: Option<char>,
    /// Visual shortcut hint string (e.g. "y", "n", "Esc").
    pub shortcut_hint: Option<String>,
    /// The value or action produced when activated.
    pub value: T,
}

/// A group of buttons with keyboard focus tracking and key reception.
#[derive(Clone, Debug, PartialEq)]
pub struct ButtonGroup<T> {
    buttons: Vec<Button<T>>,
    focused: usize,
}

// ========================================================================
// Button implementations
// ========================================================================

impl<T> Button<T> {
    /// Create a new button with `label` and `value`.
    pub fn new(label: impl Into<String>, value: T) -> Self {
        Self {
            label: label.into(),
            primary: false,
            shortcut_char: None,
            shortcut_hint: None,
            value,
        }
    }

    /// Mark this button as the primary/default choice.
    pub fn primary(mut self, primary: bool) -> Self {
        self.primary = primary;
        self
    }

    /// Bind a single-character shortcut and an optional display hint to this button.
    pub fn with_shortcut(mut self, ch: char, hint: impl Into<String>) -> Self {
        self.shortcut_char = Some(ch.to_ascii_lowercase());
        self.shortcut_hint = Some(hint.into());
        self
    }
}

// ========================================================================
// ButtonGroup implementations
// ========================================================================

impl<T: Clone> ButtonGroup<T> {
    /// Create a button group, initially focusing the primary button (or the first).
    pub fn new(buttons: Vec<Button<T>>) -> Self {
        let focused = buttons.iter().position(|b| b.primary).unwrap_or(0);
        Self { buttons, focused }
    }

    /// The list of buttons in this group.
    pub fn buttons(&self) -> &[Button<T>] {
        &self.buttons
    }

    /// Index of the currently focused button.
    pub fn focused(&self) -> usize {
        self.focused
    }

    /// Currently focused button reference, if any.
    pub fn focused_button(&self) -> Option<&Button<T>> {
        self.buttons.get(self.focused)
    }

    /// Set the focused button index.
    pub fn set_focused(&mut self, index: usize) {
        if index < self.buttons.len() {
            self.focused = index;
        }
    }

    /// Move focus to the next button, wrapping around at the end.
    pub fn focus_next(&mut self) {
        if !self.buttons.is_empty() {
            self.focused = (self.focused + 1) % self.buttons.len();
        }
    }

    /// Move focus to the previous button, wrapping around at the start.
    pub fn focus_prev(&mut self) {
        if !self.buttons.is_empty() {
            self.focused = match self.focused == 0 {
                true => self.buttons.len() - 1,
                false => self.focused - 1,
            };
        }
    }

    /// Convert buttons into renderable [`ButtonView`] items.
    pub fn to_view(&self) -> Vec<ButtonView> {
        self.buttons
            .iter()
            .enumerate()
            .map(|(i, b)| ButtonView {
                focused: i == self.focused,
                primary: b.primary,
                shortcut: b.shortcut_hint.clone(),
                text: b.label.clone(),
            })
            .collect()
    }

    /// Handle a key press directed at the button group.
    ///
    /// - `Tab` (or Right/Down arrow): cycles focus forward.
    /// - `Shift+Tab` / `BackTab` (or Left/Up arrow): cycles focus backward.
    /// - `Enter` / `Space`: activates the focused button.
    /// - Matching shortcut character (e.g. `'y'`, `'n'`): activates matching button directly.
    /// - `Escape`: activates a button bound to "Esc" or `'n'`, or the last button.
    ///
    /// Returns `Some(value)` if a button was activated, or `None` if focus changed
    /// or the key was ignored.
    pub fn on_key(&mut self, key: &Key) -> Option<T> {
        match key.chord() {
            (Mods::NONE, KeyCode::Tab | KeyCode::Right | KeyCode::Down) => {
                self.focus_next();
                None
            }
            (Mods::SHIFT, KeyCode::Tab) | (Mods::NONE, KeyCode::Left | KeyCode::Up) => {
                self.focus_prev();
                None
            }
            (Mods::NONE, KeyCode::Enter | KeyCode::Space | KeyCode::Char(' ')) => {
                self.buttons.get(self.focused).map(|b| b.value.clone())
            }
            (Mods::NONE, KeyCode::Escape) => {
                if let Some(btn) = self.buttons.iter().find(|b| {
                    b.shortcut_hint.as_deref() == Some("Esc") || b.shortcut_char == Some('n')
                }) {
                    return Some(btn.value.clone());
                }
                self.buttons.last().map(|b| b.value.clone())
            }
            (Mods::NONE, KeyCode::Char(c)) => {
                let lower = c.to_ascii_lowercase();
                self.buttons
                    .iter()
                    .find(|b| b.shortcut_char == Some(lower))
                    .map(|b| b.value.clone())
            }
            _ => None,
        }
    }
}

impl ButtonGroup<bool> {
    /// A standard confirmation button group: `Yes` (primary, 'y') and `No` ('n').
    pub fn confirm() -> Self {
        Self::new(vec![
            Button::new("Yes", true)
                .with_shortcut('y', "y")
                .primary(true),
            Button::new("No", false).with_shortcut('n', "n"),
        ])
    }
}

impl ButtonGroup<String> {
    /// Standard query-replace button group: `Yes` (y), `No` (n), `All` (!), `Stop` (q).
    pub fn query_replace() -> Self {
        Self::new(vec![
            Button::new("Yes", "y".to_string())
                .with_shortcut('y', "y")
                .primary(true),
            Button::new("No", "n".to_string()).with_shortcut('n', "n"),
            Button::new("All", "!".to_string()).with_shortcut('!', "!"),
            Button::new("Stop", "q".to_string()).with_shortcut('q', "q"),
        ])
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> Key {
        Key::plain(code)
    }

    fn shift_press(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: true,
            sup: false,
        }
    }

    #[test]
    fn test_tab_cycles_focus_forward_and_shift_tab_backward() {
        let mut group = ButtonGroup::confirm();
        assert_eq!(group.focused(), 0);

        // Tab -> focus moves to No (index 1)
        assert_eq!(group.on_key(&press(KeyCode::Tab)), None);
        assert_eq!(group.focused(), 1);

        // Tab -> wraps back to Yes (index 0)
        assert_eq!(group.on_key(&press(KeyCode::Tab)), None);
        assert_eq!(group.focused(), 0);

        // Shift+Tab -> moves back to No (index 1)
        assert_eq!(group.on_key(&shift_press(KeyCode::Tab)), None);
        assert_eq!(group.focused(), 1);

        // Shift+Tab again -> moves back to Yes (index 0)
        assert_eq!(group.on_key(&shift_press(KeyCode::Tab)), None);
        assert_eq!(group.focused(), 0);
    }

    #[test]
    fn test_arrow_keys_cycle_focus() {
        let mut group = ButtonGroup::confirm();
        assert_eq!(group.focused(), 0);

        // Right arrow moves to No
        assert_eq!(group.on_key(&press(KeyCode::Right)), None);
        assert_eq!(group.focused(), 1);

        // Left arrow moves to Yes
        assert_eq!(group.on_key(&press(KeyCode::Left)), None);
        assert_eq!(group.focused(), 0);
    }

    #[test]
    fn test_enter_activates_currently_focused_button() {
        let mut group = ButtonGroup::confirm();

        // Yes is focused initially; Enter confirms true
        assert_eq!(group.on_key(&press(KeyCode::Enter)), Some(true));

        // Move to No; Enter confirms false
        let mut group = ButtonGroup::confirm();
        group.focus_next();
        assert_eq!(group.on_key(&press(KeyCode::Enter)), Some(false));
    }

    #[test]
    fn test_shortcut_keys_activate_directly_regardless_of_focus() {
        let mut group = ButtonGroup::confirm();
        // Even when Yes is focused, pressing 'n' directly answers false
        assert_eq!(group.on_key(&press(KeyCode::Char('n'))), Some(false));

        // Even when No is focused, pressing 'y' directly answers true
        let mut group = ButtonGroup::confirm();
        group.focus_next();
        assert_eq!(group.on_key(&press(KeyCode::Char('y'))), Some(true));
    }

    #[test]
    fn test_escape_activates_cancel() {
        let mut group = ButtonGroup::confirm();
        assert_eq!(group.on_key(&press(KeyCode::Escape)), Some(false));
    }

    #[test]
    fn test_to_view_reflects_current_focus() {
        let mut group = ButtonGroup::confirm();
        let views = group.to_view();
        assert!(views[0].focused);
        assert!(!views[1].focused);

        group.focus_next();
        let views = group.to_view();
        assert!(!views[0].focused);
        assert!(views[1].focused);
    }
}
