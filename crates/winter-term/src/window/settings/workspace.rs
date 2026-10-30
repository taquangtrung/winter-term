//! The settings a workspace changes over `settings.kdl`, as VS Code's
//! workspace settings change the user's.
//!
//! What is in force is `settings.kdl` with the window's workspace's settings
//! applied over it, each through the same [`App::apply_setting`] an edit on
//! the settings page goes through. `settings.kdl` itself only ever holds the
//! user's own: an edit in the User scope is saved from the file as read, not
//! from what is in force, so a workspace's setting never leaks into it.

use std::collections::BTreeMap;

use crate::config::Config;
use crate::model::settings_page::{Scope, SettingsPage};

use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// Why a User-scope edit is not seen at once: the workspace sets it too.
const OVERRIDDEN: &str = "saved for every workspace; this one sets its own, kept in force";

// ========================================================================
// App: workspace settings
// ========================================================================

impl App {
    /// Make `name` the window's workspace, or none, and put its settings in
    /// force over `settings.kdl` in place of the last one's.
    pub(in crate::window) fn enter_workspace(&mut self, name: Option<String>) {
        let settings = name
            .as_deref()
            .and_then(|name| self.workspace.store.as_ref()?.load(name))
            .map(|workspace| workspace.settings)
            .unwrap_or_default();
        self.workspace.active = name;
        if settings.is_empty() && self.workspace.settings.is_empty() {
            return;
        }
        self.put_settings_in_force(settings);
    }

    /// Apply the window's workspace's settings over a config just read from
    /// `settings.kdl`.
    pub(in crate::window) fn apply_workspace_settings(&mut self) {
        for (key, value) in self.workspace.settings.clone() {
            self.apply_setting(&key, &value);
        }
    }

    /// Apply an edit made on the settings page in `scope`, and keep it there.
    pub(in crate::window) fn apply_scoped_edit(&mut self, scope: Scope, key: &str, value: &str) {
        match scope {
            Scope::Workspace if self.workspace.active.is_some() => {
                if self.apply_setting(key, value) {
                    self.workspace
                        .settings
                        .insert(key.to_string(), value.to_string());
                    self.save_workspace_settings();
                }
                self.dirty = true;
            }
            Scope::Workspace | Scope::User => self.apply_user_edit(key, value),
        }
    }

    /// Apply an edit made on `page`, in its scope, and mark the rows the
    /// workspace now sets.
    pub(in crate::window) fn apply_page_edit(
        &mut self,
        page: &mut SettingsPage,
        key: &str,
        value: &str,
    ) {
        self.apply_scoped_edit(page.scope, key, value);
        page.overridden = self.workspace.settings.keys().cloned().collect();
    }

    /// Show on `page` the values now in force, keeping its row and scope.
    pub(in crate::window) fn refresh_settings_page(&self, page: &mut SettingsPage) {
        let mut fresh = self.build_settings_page();
        fresh.selected = page.selected;
        fresh.scope = page.scope;
        *page = fresh;
    }

    /// Stop the window's workspace setting `key`, so `settings.kdl`'s value
    /// is in force again.
    pub(in crate::window) fn reset_workspace_setting(&mut self, key: &str) {
        if self.workspace.settings.remove(key).is_none() {
            return;
        }
        self.save_workspace_settings();
        let settings = std::mem::take(&mut self.workspace.settings);
        self.put_settings_in_force(settings);
        self.dirty = true;
    }

    /// Apply a User-scope edit and save it to `settings.kdl`. With workspace
    /// settings in force, the file is saved from what it says, read again,
    /// with the edit on it, and the workspace's settings go back over that.
    fn apply_user_edit(&mut self, key: &str, value: &str) {
        if self.workspace.settings.is_empty() {
            self.apply_settings_edit(key, value);
            return;
        }
        let (user, problem) = Config::load_checked();
        if problem.is_some() {
            // The file does not parse: as with no workspace, the edit applies
            // and the file is left for the user to fix.
            self.apply_settings_edit(key, value);
            return;
        }
        let in_force = std::mem::replace(&mut self.config, user);
        if !self.apply_setting(key, value) {
            self.config = in_force;
            return;
        }
        if let Err(e) = self.config.save() {
            self.set_error(format!("could not save settings.kdl: {e}"));
        }
        self.apply_workspace_settings();
        if self.workspace.settings.contains_key(key) {
            self.set_notice(OVERRIDDEN);
        }
        self.dirty = true;
    }

    /// Put `settings.kdl`, read again, in force with `settings` over it, as
    /// the window's workspace's settings from now on.
    fn put_settings_in_force(&mut self, settings: BTreeMap<String, String>) {
        let (user, problem) = Config::load_checked();
        // A file that does not parse is reported where it is read; what was
        // in force stays under the new settings.
        if problem.is_none() {
            self.adopt_config(user);
        }
        self.workspace.settings = settings;
        self.apply_workspace_settings();
    }

    /// Write the window's workspace's settings into its file.
    fn save_workspace_settings(&mut self) {
        let Some(name) = self.workspace.active.clone() else {
            return;
        };
        let Some(store) = &self.workspace.store else {
            return;
        };
        let Some(mut workspace) = store.load(&name) else {
            return;
        };
        workspace.settings = self.workspace.settings.clone();
        if let Err(e) = store.save(&workspace) {
            self.set_error(format!("could not save workspace {name}: {e}"));
        }
    }
}
