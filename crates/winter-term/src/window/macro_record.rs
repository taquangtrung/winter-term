//! Keyboard macro recording and replay.

use std::collections::HashMap;

use crate::model::input::{Key, MacroStep};

/// State for macro recording and playback.
#[derive(Clone, Debug, Default)]
pub(in crate::window) struct MacroState {
    /// Active recording register (`None` when not recording).
    pub recording: Option<char>,
    /// Keystrokes collected in the active recording session.
    pub buffer: Vec<Key>,
    /// The most recently recorded macro.
    pub last_macro: Vec<Key>,
    /// Named macros saved in registers ('a'..='z').
    pub registers: HashMap<char, Vec<Key>>,
    /// Depth counter to avoid runaway recursion during macro replay.
    pub replay_depth: usize,
    /// Whether the previous action was macro replay (for repeating with `e`).
    pub last_was_replay: bool,
}

impl MacroState {
    /// Whether a macro is currently being recorded.
    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }
}

// ========================================================================
// App: Macro Implementation
// ========================================================================

impl crate::window::App {
    /// Do `step` with a keyboard macro, whoever asked for it: a chord, or
    /// an editor's own `q` and `@`.
    pub(in crate::window) fn run_macro_step(&mut self, step: MacroStep) {
        match step {
            MacroStep::Record(reg) => self.start_macro_recording(reg),
            MacroStep::Replay(reg) => self.replay_macro(reg),
            MacroStep::Stop => self.stop_macro_recording(),
        }
    }

    /// Start recording keystrokes into a macro.
    pub(in crate::window) fn start_macro_recording(&mut self, reg: Option<char>) {
        if self.macro_state.is_recording() {
            self.stop_macro_recording();
        }
        let target_reg = reg.unwrap_or('@');
        self.macro_state.recording = Some(target_reg);
        self.macro_state.buffer.clear();
        self.macro_state.last_was_replay = false;
        if target_reg == '@' {
            self.set_notice("Defining keyboard macro...".to_string());
        } else {
            self.set_notice(format!("recording @{target_reg}"));
        }
        self.dirty = true;
    }

    /// Stop recording keystrokes.
    pub(in crate::window) fn stop_macro_recording(&mut self) {
        if let Some(reg) = self.macro_state.recording.take() {
            let recorded = std::mem::take(&mut self.macro_state.buffer);
            self.macro_state.last_macro = recorded.clone();
            if reg != '@' {
                self.macro_state.registers.insert(reg, recorded);
                self.set_notice(format!("macro recorded to @{reg}"));
            } else {
                self.set_notice("Keyboard macro defined".to_string());
            }
            self.macro_state.last_was_replay = false;
            self.dirty = true;
        }
    }

    /// Replay the recorded macro.
    pub(in crate::window) fn replay_macro(&mut self, reg: Option<char>) {
        if self.macro_state.is_recording() {
            self.stop_macro_recording();
        }
        if self.macro_state.replay_depth >= 16 {
            self.set_error("macro replay recursion limit reached");
            return;
        }

        let macro_keys = match reg {
            Some(r) => self.macro_state.registers.get(&r).cloned(),
            None => {
                if !self.macro_state.last_macro.is_empty() {
                    Some(self.macro_state.last_macro.clone())
                } else {
                    None
                }
            }
        };

        let Some(keys) = macro_keys else {
            self.set_error("no macro recorded");
            return;
        };

        self.macro_state.replay_depth += 1;
        self.macro_state.last_was_replay = true;

        for key in keys {
            let logical_key = match key.code { // key-exact: synthesis for winit
                crate::model::input::KeyCode::Char(c) => {
                    winit::keyboard::Key::Character(c.to_string().into())
                }
                crate::model::input::KeyCode::Enter => {
                    winit::keyboard::Key::Named(winit::keyboard::NamedKey::Enter)
                }
                crate::model::input::KeyCode::Backspace => {
                    winit::keyboard::Key::Named(winit::keyboard::NamedKey::Backspace)
                }
                crate::model::input::KeyCode::Tab => {
                    winit::keyboard::Key::Named(winit::keyboard::NamedKey::Tab)
                }
                crate::model::input::KeyCode::Escape => {
                    winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape)
                }
                _ => winit::keyboard::Key::Character("".into()),
            };
            let physical_key = winit::keyboard::PhysicalKey::Unidentified(
                winit::keyboard::NativeKeyCode::Unidentified,
            );
            self.route_key(
                winit::event::ElementState::Pressed,
                key,
                logical_key.clone(),
                physical_key,
            );
            self.route_key(
                winit::event::ElementState::Released,
                key,
                logical_key,
                physical_key,
            );
        }

        self.macro_state.replay_depth -= 1;
        self.dirty = true;
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::input::KeyCode;

    #[test]
    fn test_macro_state_default() {
        let state = MacroState::default();
        assert!(!state.is_recording());
        assert!(state.buffer.is_empty());
        assert!(state.last_macro.is_empty());
    }

    #[test]
    fn test_macro_state_recording_flag() {
        let mut state = MacroState {
            recording: Some('a'),
            ..MacroState::default()
        };
        assert!(state.is_recording());
        state.buffer.push(Key::plain(KeyCode::Char('x')));
        assert_eq!(state.buffer.len(), 1);
    }
}
