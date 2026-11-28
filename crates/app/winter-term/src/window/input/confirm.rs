//! Confirmations: the question asked before an action, by the window
//! before closing what would lose something, or by an editor before an
//! action of its own. Each is a misty [`Confirmation`], asked in one
//! [`Overlay::Confirm`], drawn by misty's `Dialog::confirm`, and answered to
//! whoever asked it ([`Asker`]).

use misty::Confirmation;
use winter_model::button::ButtonGroup;
use winter_model::input::Key;
use winter_model::layout::TabId;

use crate::render::InputView;
use crate::window::App;
use crate::window::editor::PendingClose;
use crate::window::input::overlay::{Asker, ConfirmPrompt, Overlay};

// ========================================================================
// Constants
// ========================================================================

/// The keys a confirmation answers to, for its help line.
const CONFIRM_KEYS: [(&str, &str); 3] = [
    ("Enter", "answer"),
    ("Tab", "next button"),
    ("Esc", "cancel"),
];

// ========================================================================
// App: confirmations
// ========================================================================

impl App {
    /// Ask `asked` for the window, carrying out `pending` on a yes.
    pub(in crate::window) fn confirm_for_window(
        &mut self,
        asked: Confirmation,
        pending: PendingClose,
    ) {
        self.confirm(asked, Asker::Window(pending));
    }

    /// Ask `asked` for tab `tab`'s editor, which hears the answer; it opens
    /// on the card of the transient whose choice asked it, if one did.
    pub(in crate::window) fn confirm_for_editor(&mut self, tab: TabId, asked: Confirmation) {
        let card = self.take_transient_card(tab);
        self.confirm(asked, Asker::Editor { card, tab });
    }

    /// Route one key into the confirmation up: `Tab` moves between its
    /// buttons, `Enter` presses the focused one, and their letters (`y`,
    /// `n`, `Esc`) answer at once.
    pub(in crate::window) fn handle_confirm_key(&mut self, key: &Key) {
        let Some(Overlay::Confirm(prompt)) = &mut self.parts.overlay else {
            return;
        };
        if let Some(yes) = prompt.buttons.on_key(key) {
            self.close_confirmation(yes);
        }
        self.dirty = true;
    }

    /// Press the confirmation's button at `index`, as a click does.
    pub(in crate::window) fn press_confirm_button(&mut self, index: usize) {
        let Some(Overlay::Confirm(prompt)) = &self.parts.overlay else {
            return;
        };
        if let Some(button) = prompt.buttons.buttons().get(index) {
            let yes = button.value;
            self.close_confirmation(yes);
        }
    }

    /// Tell `asker` the answer, `yes` to go ahead.
    pub(in crate::window) fn answer_confirmation(&mut self, asker: Asker, yes: bool) {
        match asker {
            Asker::Editor { tab, .. } => {
                if let Some(slot) = self.open_tabs.page_mut(&tab) {
                    let outcome = slot.page.on_confirm(yes);
                    self.act_on_editor_outcome(tab, outcome);
                }
            }
            Asker::Window(pending) => self.answer_close_prompt(pending, yes),
        }
    }

    /// The dialog for the confirmation up, if one is.
    pub(in crate::window) fn confirm_view(&self) -> Option<InputView> {
        let prompt = self.parts.confirm_prompt()?;
        let card = match prompt.then {
            Asker::Editor { card, .. } => card,
            Asker::Window(_) => None,
        };
        Some(InputView {
            buttons: prompt.buttons.to_view(),
            card,
            confirmation: Some(prompt.asked.clone()),
            keys: CONFIRM_KEYS
                .iter()
                .map(|(key, does)| (key.to_string(), does.to_string()))
                .collect(),
            kind: prompt
                .asked
                .is_destructive()
                .then_some(crate::render::NoticeKind::Warning),
            label: prompt.asked.message().to_string(),
            title: Some(prompt.asked.title().to_string()),
            ..InputView::default()
        })
    }

    /// Put `asked` up, answered to `then`.
    fn confirm(&mut self, asked: Confirmation, then: Asker) {
        let buttons = ButtonGroup::confirming(&asked);
        self.open_overlay(Overlay::Confirm(ConfirmPrompt {
            asked,
            buttons,
            then,
        }));
    }

    /// Put the confirmation away and tell its asker the answer.
    fn close_confirmation(&mut self, yes: bool) {
        let Some(Overlay::Confirm(prompt)) = self
            .parts
            .overlay
            .take_if(|overlay| matches!(overlay, Overlay::Confirm(_)))
        else {
            return;
        };
        self.dirty = true;
        self.answer_confirmation(prompt.then, yes);
    }
}
