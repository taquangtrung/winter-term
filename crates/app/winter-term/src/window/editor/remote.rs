//! The file browser over somewhere other than the local disk: as root, in a
//! Docker container, or on an SSH host (see [`winter_base::remote`]). The browser
//! is the same one; these open it at a remote path, and ask for the sudo
//! password first where browsing as root needs it.

use winter_base::remote::{self, Kind, Remote, RemoteError};
use winter_field::Field;
use winter_model::input::Key;
use winter_model::modal::EditOutcome;

use crate::window::input::overlay::{Overlay, PasswordInput};
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// What the password dialog asks.
pub(in crate::window) const PASSWORD_LABEL: &str = "Password for sudo";

// ========================================================================
// App: remote browsing
// ========================================================================

impl App {
    /// The file browser as root, over the focused pane's directory. Where
    /// sudo wants a password first, it is asked for before anything lists.
    pub(in crate::window) fn open_root_browser(&mut self) {
        if !remote::sudo_available() {
            self.set_error(remote::SUDO_UNAVAILABLE);
            return;
        }
        let start = Remote::sudo(&self.focused_start_dir()).to_path();
        match remote::sudo_ready() {
            true => self.open_browser_at(start),
            false => self.open_overlay(Overlay::Password(PasswordInput {
                field: Field::line(""),
                then: start,
            })),
        }
    }

    /// The file browser over the list of targets of `kind`, the running
    /// containers or the configured SSH hosts, to pick one and go into it.
    pub(in crate::window) fn open_target_browser(&mut self, kind: Kind) {
        if remote::targets(kind).is_empty() {
            self.refuse(match kind {
                Kind::Docker => "no running containers (is docker installed and reachable?)",
                Kind::Ssh => "no hosts in ~/.ssh/config",
            });
            return;
        }
        self.open_browser_at(remote::targets_path(kind));
    }

    /// Route one key into the password dialog: `Enter` hands the password
    /// to sudo and, once taken, opens the browser as root.
    pub(in crate::window) fn handle_password_key(&mut self, key: &Key) {
        let Some(Overlay::Password(input)) = &mut self.chrome.overlay else {
            return;
        };
        self.dirty = true;
        match input.field.on_key(&key.typed()) {
            EditOutcome::Submit => {
                let password = input.field.text().to_string();
                let then = input.then.clone();
                self.chrome.overlay = None;
                match remote::sudo_authenticate(&password) {
                    Ok(()) => self.open_browser_at(then),
                    Err(RemoteError::Failed(reason)) => self.set_error(reason),
                    Err(RemoteError::NeedsPassword) => {
                        self.set_error("sudo did not take the password")
                    }
                }
            }
            EditOutcome::Cancel => self.chrome.overlay = None,
            _ => {}
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use winter_model::input::KeyCode;

    fn press(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
            sup: false,
        }
    }

    #[test]
    fn test_the_password_dialog_shows_dots_and_escape_puts_it_away() {
        let mut app = App::new();
        app.open_overlay(Overlay::Password(PasswordInput {
            field: Field::line(""),
            then: PathBuf::from("sudo:///etc"),
        }));
        for c in "pw".chars() {
            app.handle_password_key(&press(KeyCode::Char(c)));
        }
        let view = app.input_view().expect("a dialog");
        assert_eq!(
            view.input.as_deref(),
            Some("••"),
            "how much is typed, never what"
        );
        assert_eq!(view.label, PASSWORD_LABEL);

        app.handle_password_key(&press(KeyCode::Escape));
        assert!(app.chrome.password_input().is_none());
        assert!(
            app.chrome.picker().is_none(),
            "nothing is browsed without the password"
        );
    }
}
