//! What holds the keyboard over the panes: a palette, a question, a name being
//! typed, the settings page. At most one at a time, so which of them a key goes
//! to is a single value rather than an order of checks, and one never sits
//! unseen under another waiting for an answer it cannot get.

use std::path::PathBuf;

use winter_model::button::ButtonGroup;
use winter_model::editor::PromptReply;
use winter_model::layout::TabId;
use winter_model::textedit::field::TextField;
use winter_picker::Picker;

use crate::window::editor::{ActivePick, ActivePrompt, PendingClose};
use crate::window::picker::WindowOutcome;
use crate::window::App;
use crate::window::Chrome;

// ========================================================================
// Data Structures
// ========================================================================

/// The one thing holding the keyboard over the panes, while one does.
pub(in crate::window) enum Overlay {
    /// The host's own question, before closing what would lose something.
    Close(ClosePrompt),
    /// The picker, or a list an editor asked to be picked from.
    Picker(Box<PickerOverlay>),
    /// A tab's new name being typed.
    Rename(RenameInput),
    /// The name of the theme about to be saved.
    ThemeName(TextField),
    /// The sudo password, asked before browsing as root.
    Password(PasswordInput),
    /// A question an editor asked, boxed as the largest of these.
    EditorPrompt(Box<ActivePrompt>),
    /// The Z.ai API key, for the system's secret store.
    ZaiKey(TextField),
}

/// The host asking whether to go ahead with a close, and what it would close.
/// Answered by confirmation buttons (with Tab navigation) or direct keys.
pub(in crate::window) struct ClosePrompt {
    pub(in crate::window) buttons: ButtonGroup<bool>,
    /// The question, naming what would be lost.
    pub(in crate::window) label: String,
    /// What closes on a yes.
    pub(in crate::window) pending: PendingClose,
    /// The dialog's title: what a yes gives up.
    pub(in crate::window) title: &'static str,
}

/// The picker, and the editor question it answers while it shows a list of
/// that editor's own choices.
pub(in crate::window) struct PickerOverlay {
    pub(in crate::window) picker: Box<Picker<WindowOutcome>>,
    /// The question a pick answers; `None` for the window's own pickers.
    pub(in crate::window) pick: Option<ActivePick>,
}

/// The sudo password being typed, and where the browser opens once sudo
/// takes it.
pub(in crate::window) struct PasswordInput {
    pub(in crate::window) field: TextField,
    pub(in crate::window) then: PathBuf,
}

/// A rename in progress: the tab it is for, fixed when it started so a click
/// moving the focus meanwhile cannot hand the name to another tab.
pub(in crate::window) struct RenameInput {
    /// The name as typed, edited with a field's Emacs keys.
    pub(in crate::window) field: TextField,
    pub(in crate::window) tab: TabId,
}

// ========================================================================
// Overlay Implementations
// ========================================================================

impl Overlay {
    /// Whether this is a dialog: a question, a close to confirm, a password
    /// or a name being typed, everything drawn in the middle of the window
    /// that must be answered first. A search being typed is not one: it is
    /// typed in the find popup, over the text it searches.
    fn is_dialog(&self) -> bool {
        match self {
            Self::EditorPrompt(prompt) => prompt.typed_search().is_none(),
            Self::Close(_) | Self::Password(_) | Self::ThemeName(_) | Self::ZaiKey(_) => true,
            Self::Picker(_) | Self::Rename(_) => false,
        }
    }

    /// Whether a prompt or pick of tab `tab`'s editor may take the keyboard
    /// from this one. A dialog stays up until it is answered, giving way
    /// only to the editor that asked it, asking again; the host's close
    /// question gives way to none, since it waits on its answer to close
    /// anything.
    fn yields_to_editor(&self, tab: TabId) -> bool {
        match self {
            Self::Close(_) => false,
            overlay => !overlay.is_dialog() || overlay.belongs_to(tab),
        }
    }

    /// Whether this belongs to tab `tab`, so it goes when the tab does.
    fn belongs_to(&self, tab: TabId) -> bool {
        match self {
            Self::Close(prompt) => prompt.pending.subject() == Some(tab),
            Self::Picker(overlay) => overlay.pick.as_ref().is_some_and(|pick| pick.tab == tab),
            Self::Rename(input) => input.tab == tab,
            Self::EditorPrompt(prompt) => prompt.tab == tab,
            Self::Password(_) | Self::ThemeName(_) | Self::ZaiKey(_) => false,
        }
    }

    /// The editor question this leaves unanswered when it goes, if any: the
    /// tab that asked, and the reply that tells it there was no answer.
    fn unanswered(&self) -> Option<(TabId, PromptReply)> {
        let (tab, tag) = match self {
            Self::Picker(overlay) => {
                let pick = overlay.pick.as_ref()?;
                (pick.tab, pick.tag)
            }
            Self::EditorPrompt(prompt) => (prompt.tab, prompt.request.tag),
            Self::Close(_)
            | Self::Password(_)
            | Self::Rename(_)
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
    pub(in crate::window) fn new(tab: TabId, text: &str) -> Self {
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

    // --------------------------------------------------------------------
    // Opening and closing
    // --------------------------------------------------------------------

    /// Give the keyboard to `overlay`. What held it goes, an editor question it
    /// held answered with nothing, so no editor is left waiting on a question
    /// nobody can see. An editor's own question or list cannot take the keyboard
    /// from the host's: it is answered with nothing at once instead.
    pub(in crate::window) fn open_overlay(&mut self, overlay: Overlay) {
        let asker = overlay.unanswered().map(|(tab, _)| tab);
        let refused = asker.is_some_and(|tab| {
            self.chrome
                .overlay
                .as_ref()
                .is_some_and(|open| !open.yields_to_editor(tab))
        });
        if refused {
            self.answer_unasked(&overlay);
            return;
        }
        // A dialog of the window's own waits for the one open to be
        // answered, rather than taking its place.
        if asker.is_none() && overlay.is_dialog() && self.chrome.dialog_open() {
            if let Some(displaced) = self.chrome.held.replace(overlay) {
                self.overlay_closed(displaced);
            }
            return;
        }
        // Installed before the one it replaces is told it went, so what that
        // editor does on hearing it sees the new overlay in place.
        let replaced = self.chrome.overlay.replace(overlay);
        if let Some(replaced) = replaced {
            self.overlay_closed(replaced);
        }
        self.dirty = true;
    }

    /// Open the dialog held back while another was up, once nothing is.
    pub(in crate::window) fn open_held_dialog(&mut self) {
        if self.chrome.overlay.is_some() {
            return;
        }
        if let Some(held) = self.chrome.held.take() {
            self.open_overlay(held);
        }
    }

    /// Close what holds the keyboard, saying so to whatever was waiting on
    /// it: an editor question is answered with nothing, the settings page puts
    /// back what it covered.
    pub(in crate::window) fn dismiss_overlay(&mut self) {
        if let Some(overlay) = self.chrome.overlay.take() {
            self.overlay_closed(overlay);
        }
        self.dirty = true;
    }

    /// Drop what holds the keyboard if it belongs to tab `tab`, which is
    /// going: nothing is told, as nothing is left to tell.
    pub(in crate::window) fn drop_overlay_of(&mut self, tab: TabId) {
        if self
            .chrome
            .overlay
            .as_ref()
            .is_some_and(|o| o.belongs_to(tab))
        {
            self.chrome.overlay = None;
            self.host.prompts.debounce = None;
            self.dirty = true;
        }
    }

    // --------------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------------

    /// Tell whatever was waiting on `overlay` that it is gone.
    fn overlay_closed(&mut self, overlay: Overlay) {
        if matches!(overlay, Overlay::EditorPrompt(_)) {
            self.host.prompts.debounce = None;
        }
        self.answer_unasked(&overlay);
        if let Overlay::Picker(picker) = overlay {
            self.picker_dismissed(*picker);
        }
    }

    /// Answer the editor question `overlay` holds, if any, with nothing.
    fn answer_unasked(&mut self, overlay: &Overlay) {
        let Some((tab, reply)) = overlay.unanswered() else {
            return;
        };
        let Some(slot) = self.open_tabs.page_mut(&tab) else {
            return;
        };
        let outcome = slot.page.on_prompt(reply);
        self.act_on_editor_outcome(tab, outcome);
    }
}

// ========================================================================
// Chrome
// ========================================================================

impl Chrome {
    /// The open picker, if the overlay is one.
    pub(in crate::window) fn picker(&self) -> Option<&Picker<WindowOutcome>> {
        let Some(Overlay::Picker(overlay)) = &self.overlay else {
            return None;
        };
        Some(&overlay.picker)
    }

    /// The open picker, to change it in place.
    pub(in crate::window) fn picker_mut(&mut self) -> Option<&mut Picker<WindowOutcome>> {
        let Some(Overlay::Picker(overlay)) = &mut self.overlay else {
            return None;
        };
        Some(&mut overlay.picker)
    }

    /// The editor question the open picker is collecting an answer to.
    pub(in crate::window) fn editor_pick(&self) -> Option<&ActivePick> {
        let Some(Overlay::Picker(overlay)) = &self.overlay else {
            return None;
        };
        overlay.pick.as_ref()
    }

    /// Whether a dialog is up (see [`Overlay::is_dialog`]). While one is,
    /// nothing else in the window takes a key or the pointer (see
    /// `route_modal_key` and `handle_mouse_input`), nor closes the window.
    pub(in crate::window) fn dialog_open(&self) -> bool {
        self.overlay.as_ref().is_some_and(Overlay::is_dialog)
    }

    /// The editor question being answered, if the overlay is one.
    pub(in crate::window) fn editor_prompt(&self) -> Option<&ActivePrompt> {
        let Some(Overlay::EditorPrompt(prompt)) = &self.overlay else {
            return None;
        };
        Some(prompt)
    }

    /// The host's close question, while it is asked.
    pub(in crate::window) fn close_prompt(&self) -> Option<&ClosePrompt> {
        let Some(Overlay::Close(prompt)) = &self.overlay else {
            return None;
        };
        Some(prompt)
    }

    /// The sudo password being typed.
    pub(in crate::window) fn password_input(&self) -> Option<&PasswordInput> {
        let Some(Overlay::Password(input)) = &self.overlay else {
            return None;
        };
        Some(input)
    }

    /// The Z.ai API key being typed.
    pub(in crate::window) fn zai_key_input(&self) -> Option<&TextField> {
        let Some(Overlay::ZaiKey(field)) = &self.overlay else {
            return None;
        };
        Some(field)
    }

    /// The new theme's name being typed.
    pub(in crate::window) fn theme_name_input(&self) -> Option<&TextField> {
        let Some(Overlay::ThemeName(field)) = &self.overlay else {
            return None;
        };
        Some(field)
    }

    /// The tab rename being typed.
    pub(in crate::window) fn rename_input(&self) -> Option<&RenameInput> {
        let Some(Overlay::Rename(input)) = &self.overlay else {
            return None;
        };
        Some(input)
    }

    /// Take the picker out to work on it while `self` is borrowed elsewhere;
    /// anything else that holds the keyboard stays where it is.
    pub(in crate::window) fn take_picker_overlay(&mut self) -> Option<PickerOverlay> {
        let Some(Overlay::Picker(overlay)) =
            self.overlay.take_if(|o| matches!(o, Overlay::Picker(_)))
        else {
            return None;
        };
        Some(*overlay)
    }
}
