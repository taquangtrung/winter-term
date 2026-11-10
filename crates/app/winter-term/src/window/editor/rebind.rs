//! A binding assigned in the Keys view, saved to the user's
//! `keybindings.kdl`, which the config watcher then reads back into the
//! keys in force.

use crate::config::save_command_binding;
use crate::editors::keys::KeysEditor;
use crate::window::commands::window_commands;
use crate::window::App;

// ========================================================================
// App: rebinding
// ========================================================================

impl App {
    /// Bind `command` of `block` to `chords` in the user's keybindings, or
    /// with none take their binding of it away, and say what was done.
    pub(in crate::window) fn save_binding(
        &mut self,
        block: &str,
        command: &str,
        chords: &[String],
        platform: Option<&str>,
    ) {
        match save_command_binding(block, command, chords, platform) {
            Ok(()) => match chords.first() {
                Some(chord) => self.set_notice(format!("{chord} now runs {command}")),
                None => self.set_notice(format!("{command} is back on its built-in keys")),
            },
            Err(reason) => self.set_error(reason),
        }
    }

    /// List the bindings now in force in every open Keys view, since a
    /// change to `keybindings.kdl` changes what they list.
    pub(in crate::window) fn refresh_keys_editors(&mut self) {
        let commands = window_commands(&self.keymap);
        for page in self.open_tabs.own_pages_mut() {
            page.with_kind_mut(|keys: &mut KeysEditor| {
                keys.refresh(&self.keymap, commands.clone(), &self.config.keybindings)
            });
        }
    }
}
