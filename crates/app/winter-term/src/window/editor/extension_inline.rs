//! The extensions' inline sources (see `winter_ext_api::InlineProvider`), as
//! the window carries them between the text editors and the extensions:
//! what each offers, handed to the editors with their settings; what the
//! editors ask and tell them, handed to the extensions with a host; and
//! what the extensions answer, handed back to the editor that asked.

use std::collections::HashMap;

use winter_ext_api::{InlineOffer, ID_SEPARATOR};
use winter_text::settings::{EditorSettings, InlineOffered};

use crate::window::App;

// ========================================================================
// Data Structures
// ========================================================================

/// What each extension's inline source offers, by its full id, as its
/// extension last said.
#[derive(Default)]
pub(in crate::window) struct InlineOffers(HashMap<String, InlineOffer>);

// ========================================================================
// App: extensions' inline sources
// ========================================================================

impl App {
    /// The settings the text editors follow: the file's, and what each
    /// extension's inline source offers now.
    pub(in crate::window) fn editor_settings(&self) -> EditorSettings {
        let mut settings = self.config.editor_settings();
        let offers = &self.host.lsp.inline_offers.0;
        settings.inline = crate::extensions::registry()
            .inlines()
            .iter()
            .map(|inline| InlineOffered {
                id: inline.id.clone(),
                offer: offers.get(&inline.id).copied().unwrap_or_default(),
                servers: self
                    .host
                    .lsp
                    .extension_servers
                    .running_ids(inline.extension),
                title: inline.title.clone(),
            })
            .collect();
        settings
    }

    /// Take `offer` as what `extension`'s inline source `name` offers.
    pub(in crate::window) fn offer_extension_inline(
        &mut self,
        extension: &str,
        name: &str,
        offer: InlineOffer,
    ) {
        let id = format!("{extension}{ID_SEPARATOR}{name}");
        self.host.lsp.inline_offers.0.insert(id, offer);
        self.dirty = true;
    }
}
