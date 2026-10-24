//! What holds the keyboard over the panes: a palette, a question, a name being
//! typed, the settings page. At most one at a time, so which of them a key goes
//! to is a single value rather than an order of checks, and one never sits
//! unseen under another waiting for an answer it cannot get.

use std::path::PathBuf;

use crate::model::button::ButtonGroup;
use crate::model::editor::PromptReply;
use crate::model::layout::TabId;
use crate::model::palette::Palette;
use crate::model::settings_page::SettingsPage;
use crate::model::textedit::field::TextField;

use super::editor::{ActivePick, ActivePrompt, PendingClose};
use super::App;

// ========================================================================
// Data Structures
// ========================================================================

/// The one thing holding the keyboard over the panes, while one does.
pub(crate) enum Overlay {
    /// The host's own question, before closing what would lose something.
    Close(ClosePrompt),
    /// The command palette, or a list an editor asked to be picked from.
    Palette(PaletteOverlay),
    /// A tab's new name being typed.
    Rename(RenameInput),
    /// The full-window settings page.
    Settings(Box<SettingsPage>),
    /// The name of the theme about to be saved.
    ThemeName(TextField),
    /// The sudo password, asked before browsing as root.
    Password(PasswordInput),
    /// A question an editor asked.
    EditorPrompt(ActivePrompt),
    /// The Z.ai API key, for the system's secret store.
    ZaiKey(TextField),
}

/// The host asking whether to go ahead with a close, and what it would close.
/// Answered by confirmation buttons (with Tab navigation) or direct keys.
pub(crate) struct ClosePrompt {
    pub(crate) buttons: ButtonGroup<bool>,
    /// The question, naming what would be lost.
    pub(crate) label: String,
    /// What closes on a yes.
    pub(crate) pending: PendingClose,
}

/// The palette, and the editor question it answers while it shows a list of
/// that editor's own choices.
pub(crate) struct PaletteOverlay {
    pub(crate) palette: Palette,
    /// The question a pick answers; `None` for the window's own palettes.
    pub(crate) pick: Option<ActivePick>,
}

/// The sudo password being typed, and where the browser opens once sudo
/// takes it.
pub(crate) struct PasswordInput {
    pub(crate) field: TextField,
    pub(crate) then: PathBuf,
}

/// A rename in progress: the tab it is for, fixed when it started so a click
/// moving the focus meanwhile cannot hand the name to another tab.
pub(crate) struct RenameInput {
    /// The name as typed, edited with a field's Emacs keys.
    pub(crate) field: TextField,
    pub(crate) tab: TabId,
}

// ========================================================================
// Overlay Implementations
// ========================================================================

impl Overlay {
    /// Whether an editor's prompt or pick may take the keyboard from this one.
    /// The host's question may not lose it to one: the answer it is waiting
    /// on decides whether anything is closed.
    fn yields_to_editor(&self) -> bool {
        !matches!(self, Self::Close(_))
    }

    /// Whether this belongs to tab `tab`, so it goes when the tab does.
    fn belongs_to(&self, tab: TabId) -> bool {
        match self {
            Self::Close(prompt) => prompt.pending.subject() == Some(tab),
            Self::Palette(overlay) => overlay.pick.as_ref().is_some_and(|pick| pick.tab == tab),
            Self::Rename(input) => input.tab == tab,
            Self::EditorPrompt(prompt) => prompt.tab == tab,
            Self::Password(_) | Self::Settings(_) | Self::ThemeName(_) | Self::ZaiKey(_) => false,
        }
    }

    /// The editor question this leaves unanswered when it goes, if any: the
    /// tab that asked, and the reply that tells it there was no answer.
    fn unanswered(&self) -> Option<(TabId, PromptReply)> {
        let (tab, tag) = match self {
            Self::Palette(PaletteOverlay {
                pick: Some(pick), ..
            }) => (pick.tab, pick.tag),
            Self::EditorPrompt(prompt) => (prompt.tab, prompt.request.tag),
            Self::Close(_)
            | Self::Palette(_)
            | Self::Password(_)
            | Self::Rename(_)
            | Self::Settings(_)
            | Self::ThemeName(_)
            | Self::ZaiKey(_) => return None,
        };
        Some((tab, PromptReply { answer: None, tag }))
    }
}

// ========================================================================
// RenameInput Implementations
// ========================================================================

impl RenameInput {
    /// A rename of tab `tab`, starting from the name `text` it has.
    pub(crate) fn new(tab: TabId, text: &str) -> Self {
        Self {
            field: TextField::line(text),
            tab,
        }
    }
}

// ========================================================================
// App: overlays
// ========================================================================

impl App {
    // --------------------------------------------------------------------
    // Reading
    // --------------------------------------------------------------------

    /// The open palette, if the overlay is one.
    pub(crate) fn palette(&self) -> Option<&Palette> {
        let Some(Overlay::Palette(overlay)) = &self.overlay else {
            return None;
        };
        Some(&overlay.palette)
    }

    /// The open palette, to change it in place.
    pub(crate) fn palette_mut(&mut self) -> Option<&mut Palette> {
        let Some(Overlay::Palette(overlay)) = &mut self.overlay else {
            return None;
        };
        Some(&mut overlay.palette)
    }

    /// The editor question the open palette is collecting an answer to.
    pub(crate) fn editor_pick(&self) -> Option<&ActivePick> {
        let Some(Overlay::Palette(overlay)) = &self.overlay else {
            return None;
        };
        overlay.pick.as_ref()
    }

    /// The editor question being answered, if the overlay is one.
    pub(crate) fn editor_prompt(&self) -> Option<&ActivePrompt> {
        let Some(Overlay::EditorPrompt(prompt)) = &self.overlay else {
            return None;
        };
        Some(prompt)
    }

    /// The host's close question, while it is asked.
    pub(crate) fn close_prompt(&self) -> Option<&ClosePrompt> {
        let Some(Overlay::Close(prompt)) = &self.overlay else {
            return None;
        };
        Some(prompt)
    }

    /// The sudo password being typed.
    pub(crate) fn password_input(&self) -> Option<&PasswordInput> {
        let Some(Overlay::Password(input)) = &self.overlay else {
            return None;
        };
        Some(input)
    }

    /// The Z.ai API key being typed.
    pub(crate) fn zai_key_input(&self) -> Option<&TextField> {
        let Some(Overlay::ZaiKey(field)) = &self.overlay else {
            return None;
        };
        Some(field)
    }

    /// The new theme's name being typed.
    pub(crate) fn theme_name_input(&self) -> Option<&TextField> {
        let Some(Overlay::ThemeName(field)) = &self.overlay else {
            return None;
        };
        Some(field)
    }

    /// The tab rename being typed.
    pub(crate) fn rename_input(&self) -> Option<&RenameInput> {
        let Some(Overlay::Rename(input)) = &self.overlay else {
            return None;
        };
        Some(input)
    }

    /// The settings page, while it is open.
    pub(crate) fn settings_page(&self) -> Option<&SettingsPage> {
        let Some(Overlay::Settings(page)) = &self.overlay else {
            return None;
        };
        Some(page)
    }

    // --------------------------------------------------------------------
    // Opening and closing
    // --------------------------------------------------------------------

    /// Show `palette`, in place of whatever holds the keyboard now.
    pub(crate) fn open_palette(&mut self, palette: Palette) {
        self.open_overlay(Overlay::Palette(PaletteOverlay {
            palette,
            pick: None,
        }));
    }

    /// Give the keyboard to `overlay`. What held it goes, an editor question it
    /// held answered with nothing, so no editor is left waiting on a question
    /// nobody can see. An editor's own question or list cannot take the keyboard
    /// from the host's: it is answered with nothing at once instead.
    pub(crate) fn open_overlay(&mut self, overlay: Overlay) {
        let from_editor = overlay.unanswered().is_some();
        if from_editor && !self.overlay.as_ref().is_none_or(Overlay::yields_to_editor) {
            self.answer_unasked(&overlay);
            return;
        }
        // Installed before the one it replaces is told it went, so what that
        // editor does on hearing it sees the new overlay in place.
        let replaced = self.overlay.replace(overlay);
        if let Some(replaced) = replaced {
            self.overlay_closed(replaced);
        }
        self.dirty = true;
    }

    /// Close what holds the keyboard, saying so to whatever was waiting on
    /// it: an editor question is answered with nothing, the settings page puts
    /// back what it covered.
    pub(crate) fn dismiss_overlay(&mut self) {
        if let Some(overlay) = self.overlay.take() {
            self.overlay_closed(overlay);
        }
        self.dirty = true;
    }

    /// Drop what holds the keyboard if it belongs to tab `tab`, which is
    /// going: nothing is told, as nothing is left to tell.
    pub(crate) fn drop_overlay_of(&mut self, tab: TabId) {
        if self.overlay.as_ref().is_some_and(|o| o.belongs_to(tab)) {
            self.overlay = None;
            self.prompt_debounce = None;
            self.dirty = true;
        }
    }

    /// Take the palette out to work on it while `self` is borrowed elsewhere;
    /// anything else that holds the keyboard stays where it is.
    pub(crate) fn take_palette_overlay(&mut self) -> Option<PaletteOverlay> {
        let Some(Overlay::Palette(overlay)) =
            self.overlay.take_if(|o| matches!(o, Overlay::Palette(_)))
        else {
            return None;
        };
        Some(overlay)
    }

    /// Put back a palette taken out with [`Self::take_palette_overlay`],
    /// unless it closed, or something else took the keyboard meanwhile (a
    /// command run from it opening a palette of its own). A pick that goes
    /// unanswered either way is answered with nothing.
    pub(crate) fn restore_palette_overlay(&mut self, overlay: PaletteOverlay) {
        if overlay.palette.active && self.overlay.is_none() {
            self.overlay = Some(Overlay::Palette(overlay));
            return;
        }
        self.overlay_closed(Overlay::Palette(overlay));
    }

    /// Take the settings page out to edit it while `self` is borrowed.
    pub(crate) fn take_settings_page(&mut self) -> Option<Box<SettingsPage>> {
        let Some(Overlay::Settings(page)) =
            self.overlay.take_if(|o| matches!(o, Overlay::Settings(_)))
        else {
            return None;
        };
        Some(page)
    }

    // --------------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------------

    /// Tell whatever was waiting on `overlay` that it is gone.
    fn overlay_closed(&mut self, overlay: Overlay) {
        if matches!(overlay, Overlay::EditorPrompt(_)) {
            self.prompt_debounce = None;
        }
        if matches!(overlay, Overlay::Settings(_)) {
            self.on_settings_closed();
        }
        self.answer_unasked(&overlay);
    }

    /// Answer the editor question `overlay` holds, if any, with nothing.
    fn answer_unasked(&mut self, overlay: &Overlay) {
        let Some((tab, reply)) = overlay.unanswered() else {
            return;
        };
        let Some(slot) = self.open_tabs.editor_mut(&tab) else {
            return;
        };
        let outcome = slot.editor.on_prompt(reply);
        self.act_on_editor_outcome(tab, outcome);
    }
}
