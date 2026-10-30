//! The status bar's AI quotas, from the window's side: which providers are
//! asked, asking them again, and taking the Z.ai API key into the system's
//! secret store.

use crate::model::input::Key;
use crate::model::textedit::modal::EditOutcome;
use crate::quota::{secret, Monitor, Provider};

use super::quota_providers;
use crate::window::input::overlay::Overlay;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// What the Z.ai key dialog asks.
pub(in crate::window) const ZAI_KEY_LABEL: &str = "Z.ai API key (blank to forget it)";

/// Said once the key is kept.
const ZAI_KEY_SAVED: &str = "Z.ai API key saved";

/// Said once a blank key has forgotten the one kept.
const ZAI_KEY_FORGOTTEN: &str = "Z.ai API key forgotten";

// ========================================================================
// App: quotas
// ========================================================================

impl App {
    /// Keep the quota monitor asking exactly the providers the status bar's
    /// segments show, starting a fresh one when they change and none when
    /// they name no provider. Nothing starts before the bar is first shown,
    /// and a hidden bar keeps what it was asking.
    pub(in crate::window) fn sync_quota_monitor(&mut self, status_enabled: bool) {
        if !status_enabled {
            return;
        }
        let wanted = quota_providers(&self.config.status_bar.segments);
        if self
            .quota
            .as_ref()
            .is_some_and(|m| *m.providers() == wanted)
        {
            return;
        }
        self.quota = (!wanted.is_empty()).then(|| Monitor::start(&self.runtime.waker, wanted));
    }

    /// Ask every provider the status bar shows for its quota again now.
    pub(in crate::window) fn refresh_quotas(&mut self) {
        if let Some(monitor) = &self.quota {
            for &provider in monitor.providers() {
                monitor.refresh(provider);
            }
        }
    }

    /// Route one key into the Z.ai key dialog: `Enter` keeps what was typed
    /// in the secret store and asks Z.ai with it at once.
    pub(in crate::window) fn handle_zai_key_key(&mut self, key: &Key) {
        let Some(Overlay::ZaiKey(field)) = &mut self.chrome.overlay else {
            return;
        };
        self.dirty = true;
        match field.on_key(&key.typed(), 1) {
            EditOutcome::Submit => {
                let typed = field.text().to_string();
                self.chrome.overlay = None;
                match secret::set_zai_key(&typed) {
                    Ok(()) if typed.trim().is_empty() => self.set_notice(ZAI_KEY_FORGOTTEN),
                    Ok(()) => self.set_notice(ZAI_KEY_SAVED),
                    Err(reason) => return self.set_error(reason),
                }
                if let Some(monitor) = &self.quota {
                    monitor.refresh(Provider::Zai);
                }
            }
            EditOutcome::Cancel => self.chrome.overlay = None,
            _ => {}
        }
    }
}
