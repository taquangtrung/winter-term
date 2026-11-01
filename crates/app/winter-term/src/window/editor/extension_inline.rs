//! The extensions' inline sources (see `winter_ext_api::InlineProvider`), as
//! the window carries them between the text editors and the extensions:
//! what each offers, handed to the editors with their settings; what the
//! editors ask and tell them, handed to the extensions with a host; and
//! what the extensions answer, handed back to the editor that asked.

use std::collections::HashMap;

use winter_ext_api::{InlineAnswer, InlineOffer, ID_SEPARATOR};
use winter_text::settings::{EditorSettings, InlineOffered};
use winter_text::{InlineCall, TextEditor};

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

    /// Hand the answer an extension gave under `ticket` to the editor that
    /// asked for it.
    pub(in crate::window) fn answer_extension_inline(
        &mut self,
        ticket: u64,
        answer: &InlineAnswer,
    ) {
        for slot in self.open_tabs.buffers_mut() {
            let taken = slot
                .editor
                .with_kind_mut(|text: &mut TextEditor| text.take_inline_answer(ticket, answer));
            if taken == Some(true) {
                self.dirty = true;
                return;
            }
        }
    }

    /// Hand the extensions what the text editors ask and tell their inline
    /// sources. Says whether there was any.
    pub(in crate::window) fn call_extension_inlines(&mut self) -> bool {
        let calls: Vec<InlineCall> = self
            .open_tabs
            .buffers_mut()
            .filter_map(|slot| slot.editor.with_kind_mut(TextEditor::take_inline_calls))
            .flatten()
            .collect();
        let any = !calls.is_empty();
        let registry = crate::extensions::registry();
        let focused = self.layout().focused();
        for call in calls {
            let source = match &call {
                InlineCall::Ask(ask) => &ask.source,
                InlineCall::Shown(told) | InlineCall::Taken(told) => &told.source,
            };
            let Some(inline) = registry.find_inline(source) else {
                continue;
            };
            let provider = inline.provider.as_ref();
            self.as_extension(inline.extension, focused, |host| match &call {
                InlineCall::Ask(ask) => provider.ask(host, &ask.request),
                InlineCall::Shown(told) => provider.shown(host, told.kind, &told.receipt),
                InlineCall::Taken(told) => provider.taken(host, told.kind, &told.receipt),
            });
        }
        any
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::extensions::tests::GHOSTED;
    use crate::window::editor::EditorKind;
    use winter_model::input::{Key, KeyCode};

    #[test]
    fn test_an_extension_s_ghost_text_shows_in_the_editor_that_asked() {
        let dir = std::env::temp_dir().join(format!("winter-ext-inline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("notes.txt");
        std::fs::write(&file, "say \n").expect("file");
        let mut app = App::new();
        let editor = TextEditor::new(file, None).expect("opens");
        let tab = app.show_editor(EditorKind::Text, Box::new(editor));
        // Heard its settings, the probe offers ghost text.
        app.watch_for_extensions();
        let settings = app.editor_settings();
        let offered = settings
            .inline
            .iter()
            .find(|offered| offered.id == "probe.ghost")
            .expect("handed to the editors");
        assert!(offered.offer.ghost && !offered.offer.edit);
        let slot = app.open_tabs.page_mut(&tab).expect("an editor");
        slot.page
            .with_kind_mut(|text: &mut TextEditor| text.configure(&settings));
        let key = |code, ctrl| Key {
            alt: false,
            code,
            ctrl,
            shift: false,
            sup: false,
        };
        slot.page.on_key(&key(KeyCode::Char('A'), false));
        slot.page.on_key(&key(KeyCode::Char('\\'), true));
        assert!(app.call_extension_inlines(), "the editor's ask handed on");
        let slot = app.open_tabs.page_mut(&tab).expect("an editor");
        slot.page.on_key(&key(KeyCode::Tab, false));
        let (_, text) = slot
            .page
            .as_editor()
            .and_then(|editor| editor.document())
            .expect("a document");
        assert_eq!(
            text.lines().next(),
            Some(format!("say {GHOSTED}").as_str()),
            "shown, and taken"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
