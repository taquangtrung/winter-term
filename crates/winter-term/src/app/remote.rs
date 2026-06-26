//! The file browser over somewhere other than the local disk: as root, in a
//! Docker container, or on an SSH host (see [`crate::remote`]). The browser
//! is the same one; these open it at a remote path, and ask for the sudo
//! password first where browsing as root needs it.

use crate::model::input::Key;
use crate::model::palette::Palette;
use crate::model::textedit::field::TextField;
use crate::model::textedit::modal::EditOutcome;
use crate::remote::{self, Kind, Remote, RemoteError};

use super::overlay::{Overlay, PasswordInput};
use super::App;

// ========================================================================
// Constants
// ========================================================================

/// What the password dialog asks.
pub(super) const PASSWORD_LABEL: &str = "Password for sudo";

// ========================================================================
// App: remote browsing
// ========================================================================

impl App {
    /// The file browser as root, over the focused pane's directory. Where
    /// sudo wants a password first, it is asked for before anything lists.
    pub(crate) fn open_root_browser(&mut self) {
        if !remote::sudo_available() {
            self.set_error(remote::SUDO_UNAVAILABLE);
            return;
        }
        let start = Remote::sudo(&self.focused_start_dir()).to_path();
        match remote::sudo_ready() {
            true => self.open_palette(Palette::open_files(start)),
            false => self.open_overlay(Overlay::Password(PasswordInput {
                field: TextField::line(""),
                then: start,
            })),
        }
    }

    /// The file browser over the list of targets of `kind`, the running
    /// containers or the configured SSH hosts, to pick one and go into it.
    pub(crate) fn open_target_browser(&mut self, kind: Kind) {
        let palette = Palette::open_files(remote::targets_path(kind));
        // The one row of an empty list is the way back up.
        if palette.entries.len() <= 1 {
            self.set_error(match kind {
                Kind::Docker => "no running containers (is docker installed and reachable?)",
                Kind::Ssh => "no hosts in ~/.ssh/config",
            });
            return;
        }
        self.open_palette(palette);
    }

    /// Route one key into the password dialog: `Enter` hands the password
    /// to sudo and, once taken, opens the browser as root.
    pub(crate) fn handle_password_key(&mut self, key: &Key) {
        let Some(Overlay::Password(input)) = &mut self.overlay else {
            return;
        };
        self.dirty = true;
        match input.field.on_key(&key.typed(), 1) {
            EditOutcome::Submit => {
                let password = input.field.text().to_string();
                let then = input.then.clone();
                self.overlay = None;
                match remote::sudo_authenticate(&password) {
                    Ok(()) => self.open_palette(Palette::open_files(then)),
                    Err(RemoteError::Failed(reason)) => self.set_error(reason),
                    Err(RemoteError::NeedsPassword) => {
                        self.set_error("sudo did not take the password")
                    }
                }
            }
            EditOutcome::Cancel => self.overlay = None,
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
    use crate::model::input::KeyCode;

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
            field: TextField::line(""),
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
        assert!(app.password_input().is_none());
        assert!(
            app.palette().is_none(),
            "nothing is browsed without the password"
        );
    }
}
