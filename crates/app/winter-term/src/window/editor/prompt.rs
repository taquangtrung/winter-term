//! Questions an editor asks: the answer typed into a dialog or picked from
//! the palette, the keys that edit it, and what the renderer is handed to
//! draw it.

use std::time::{Duration, Instant};

use crate::render::{ButtonView, InputView};
use winter_field::Field;
use winter_model::button::ButtonGroup;
use winter_model::editor::{PickPlacement, PromptMode, PromptReply, PromptRequest, SearchReport};
use winter_model::input::{Key, KeyCode, Mods};
use winter_model::layout::TabId;
use winter_model::modal::EditOutcome;
use winter_model::recall::{Answers, Recall};

use crate::window::EditorHost;
use crate::window::input::overlay::Overlay;
use crate::window::{App, SEARCH_DEBOUNCE_MILLIS};

// ========================================================================
// Constants
// ========================================================================

/// The keys that answer an error's report.
const ERROR_KEYS: [(&str, &str); 2] = [("Enter/Esc", "OK"), ("Tab", "next button")];

/// Key hints for interactive query replace.
const QUERY_REPLACE_KEYS: [(&str, &str); 5] = [
    ("y/Spc", "Yes"),
    ("n/Del", "No"),
    ("!", "All"),
    ("./Ret", "Replace & quit"),
    ("q/Esc", "Quit"),
];

/// How a question taken by one key says so in its own words, which its
/// buttons now say instead.
const YES_NO_SUFFIX: &str = "(y/n)";

/// What the question naming a theme being saved asks.
const THEME_NAME_LABEL: &str = "New theme name";

/// The keys that answer a question typed on a line, and what each does.
const TEXT_KEYS: [(&str, &str); 2] = [("Enter", "accept"), ("Esc", "cancel")];

/// What each character of a secret answer is drawn as, so the dialog shows
/// how much has been typed and never what.
const SECRET_MASK: char = '•';

// ========================================================================
// Data Structures
// ========================================================================

/// What the window keeps for the questions editors ask.
#[derive(Default)]
pub(in crate::window) struct Prompts {
    /// What each question was answered with before, for the arrow keys.
    answers: Answers,
    /// When what is typed into a question is next handed to the editor that
    /// asked, and which tab that is: the typing pauses first.
    pub(in crate::window) debounce: Option<(Instant, TabId)>,
}

/// A question an editor asked, and the answer being typed for it.
pub(in crate::window) struct ActivePrompt {
    /// Optional button group for interactive query replace.
    pub(in crate::window) query_buttons: Option<ButtonGroup<String>>,
    /// What has been typed, edited with the same keys as any other text.
    pub(in crate::window) field: Field,
    /// The card of the transient whose choice asked, which the dialog opens
    /// on; `None` for one asked otherwise.
    pub(in crate::window) origin: Option<(f32, f32, f32, f32)>,
    /// How far the arrow keys have walked back through earlier answers.
    pub(in crate::window) recall: Recall,
    /// The tab that asked.
    pub(in crate::window) tab: TabId,
    pub(in crate::window) request: PromptRequest,
}

/// The editor question the palette is standing in for while it shows a list of
/// that editor's own choices.
pub(in crate::window) struct ActivePick {
    /// The card of the transient whose choice asked for the list, which the
    /// list is drawn on in place of `placement`; `None` for one asked otherwise.
    pub(in crate::window) origin: Option<(f32, f32, f32, f32)>,
    /// Where the list is shown: centred, or floating at the asking
    /// editor's cursor.
    pub(in crate::window) placement: PickPlacement,
    /// The editor that asked.
    pub(in crate::window) tab: TabId,
    /// The question being answered, carried back with the choice.
    pub(in crate::window) tag: &'static str,
}

/// Where a transient was drawn when a key closed it, held until its editor
/// next asks for a list or is handed another key.
pub(in crate::window) struct TransientCard {
    /// The card, as `(x, y, w, h)` in surface pixels.
    pub(in crate::window) rect: (f32, f32, f32, f32),
    /// The editor whose transient it was.
    pub(in crate::window) tab: TabId,
}

// ========================================================================
// ActivePrompt Implementations
// ========================================================================

impl ActivePrompt {
    /// Create an active prompt for `request` on `tab`.
    pub(in crate::window) fn new(tab: TabId, request: PromptRequest) -> Self {
        let query_buttons = match request.mode {
            PromptMode::QueryReplace => Some(ButtonGroup::query_replace()),
            _ => None,
        };
        Self {
            query_buttons,
            field: Field::line(&request.initial),
            origin: None,
            recall: Recall::default(),
            tab,
            request,
        }
    }

    /// What a search question has had typed into it so far, and whether it
    /// runs backwards. `None` for any other question.
    pub(in crate::window) fn typed_search(&self) -> Option<(String, bool)> {
        match self.request.mode {
            PromptMode::Search { reverse } => Some((self.field.text(), reverse)),
            PromptMode::QueryReplace | PromptMode::Secret | PromptMode::Text => None,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The dialog for one question: what is asked, the line it is answered on for
/// a question that takes one, and how to answer it. A question answered by a
/// single key has no line to type on, so it is given none.
/// Which way a key walks a prompt's earlier answers: `Some(true)` back to an
/// older one (Up, `C-p`), `Some(false)` on to a newer (Down, `C-n`).
fn recall_step(key: &Key) -> Option<bool> {
    match key.chord() {
        (Mods::NONE, KeyCode::Up) | (Mods::CTRL, KeyCode::Char('p')) => Some(true),
        (Mods::NONE, KeyCode::Down) | (Mods::CTRL, KeyCode::Char('n')) => Some(false),
        _ => None,
    }
}

pub(in crate::window) fn input_dialog(
    label: &str,
    input: &str,
    caret: usize,
    mode: PromptMode,
    buttons: Vec<ButtonView>,
) -> InputView {
    InputView {
        buttons,
        caret,
        caret_visible: true,
        input: match mode {
            PromptMode::QueryReplace => None,
            PromptMode::Secret => Some(input.chars().map(|_| SECRET_MASK).collect()),
            PromptMode::Search { .. } | PromptMode::Text => Some(input.to_string()),
        },
        keys: match mode {
            PromptMode::QueryReplace => &QUERY_REPLACE_KEYS[..],
            PromptMode::Search { .. } | PromptMode::Secret | PromptMode::Text => &TEXT_KEYS[..],
        }
        .iter()
        .map(|(key, does)| (key.to_string(), does.to_string()))
        .collect(),
        // Labels are written to sit before an answer on one line, so they end
        // in a separator that reads as a dangling colon over one, or in a
        // `(y/n)` the buttons say instead.
        label: label
            .trim_end()
            .trim_end_matches(YES_NO_SUFFIX)
            .trim_end()
            .trim_end_matches(':')
            .to_string(),
        ..InputView::default()
    }
}

// ========================================================================
// App: editor prompts
// ========================================================================

impl App {
    /// Hand the typing to the editor whose question it is, once its pause
    /// is up. When that will be, where it is still to come.
    pub(in crate::window) fn settle_prompt_input(&mut self, now: Instant) -> Option<Instant> {
        let (deadline, tab) = self.host.prompts.debounce?;
        if now < deadline {
            return Some(deadline);
        }
        self.host.prompts.debounce = None;
        if let Some(prompt) = self.parts.editor_prompt() {
            let query = prompt.field.text().to_string();
            let tag = prompt.request.tag;
            if let Some(slot) = self.open_tabs.page_mut(&tab) {
                let outcome = slot.page.on_prompt_input(&query, tag);
                self.act_on_editor_outcome(tab, outcome);
            }
        }
        self.dirty = true;
        None
    }

    /// Route one key into the open prompt: `Enter` answers, `Esc` cancels, and
    /// a confirm prompt resolves on the first key it sees.
    pub(in crate::window) fn handle_prompt_key(&mut self, key: &Key) {
        let key = &key.typed();
        let mut debounce_from = None;
        let answer = {
            // The field rather than the accessor, so the answers kept beside
            // it can be read while the prompt is held.
            let Some(Overlay::EditorPrompt(prompt)) = &mut self.parts.overlay else {
                return;
            };
            match (prompt.request.mode, key.chord()) {
                (PromptMode::QueryReplace, _) => match key.chord() {
                    (Mods::NONE, KeyCode::Escape)
                    | (Mods::CTRL, KeyCode::Char('g'))
                    | (Mods::NONE, KeyCode::Char('q' | 'Q')) => Some(Some("q".to_string())),
                    (Mods::NONE, KeyCode::Char('y' | 'Y' | ' ')) => Some(Some("y".to_string())),
                    (
                        Mods::NONE,
                        KeyCode::Char('n' | 'N') | KeyCode::Backspace | KeyCode::Delete,
                    ) => Some(Some("n".to_string())),
                    (Mods::NONE, KeyCode::Char('!')) => Some(Some("!".to_string())),
                    (Mods::NONE, KeyCode::Char('.')) | (Mods::NONE, KeyCode::Enter) => {
                        Some(Some(".".to_string()))
                    }
                    _ => {
                        if let Some(buttons) = &mut prompt.query_buttons {
                            buttons.on_key(key).map(Some)
                        } else {
                            None
                        }
                    }
                },
                (_, (Mods::NONE, KeyCode::Escape)) => Some(None),
                // A password is edited like any answer, but is neither
                // recorded for recall nor handed to the editor while typed.
                (PromptMode::Secret, _) => match prompt.field.on_key(key) {
                    EditOutcome::Submit => Some(Some(prompt.field.text().to_string())),
                    EditOutcome::Cancel => Some(None),
                    _ => None,
                },
                // Up and Down walk back through what this question was
                // answered with before, as a shell's history does.
                (PromptMode::Search { .. } | PromptMode::Text, _) if recall_step(key).is_some() => {
                    let answers = self.host.prompts.answers.of(prompt.request.tag);
                    let typed = prompt.field.text().to_string();
                    let recalled = match recall_step(key) {
                        Some(true) => prompt.recall.older(answers, &typed),
                        _ => prompt.recall.newer(answers),
                    };
                    if let Some(text) = recalled {
                        prompt.field.set_text(&text);
                        debounce_from = Some(prompt.tab);
                    }
                    None
                }
                // The answer is a line of text like any other, so it takes
                // the same Emacs keys a file does while typing.
                (PromptMode::Search { .. } | PromptMode::Text, _) => {
                    match prompt.field.on_key(key) {
                        EditOutcome::Submit => {
                            let answer = prompt.field.text().to_string();
                            self.host
                                .prompts
                                .answers
                                .record(prompt.request.tag, &answer);
                            Some(Some(answer))
                        }
                        EditOutcome::Cancel => Some(None),
                        outcome => {
                            if outcome != EditOutcome::Ignored {
                                debounce_from = Some(prompt.tab);
                            }
                            None
                        }
                    }
                }
            }
        };
        self.dirty = true;
        if let Some(tab) = debounce_from {
            self.host.debounce_prompt_input(tab);
        }
        let Some(answer) = answer else {
            return;
        };
        self.term.search.match_index = 0;
        self.term.search.match_total = 0;
        self.term.search.current = None;
        self.host.prompts.debounce = None;
        let Some(Overlay::EditorPrompt(prompt)) = self
            .parts
            .overlay
            .take_if(|o| matches!(o, Overlay::EditorPrompt(_)))
        else {
            return;
        };
        let tab_id = prompt.tab;
        let reply = PromptReply {
            answer,
            tag: prompt.request.tag,
        };
        let Some(slot) = self.open_tabs.page_mut(&tab_id) else {
            return;
        };
        let outcome = slot.page.on_prompt(reply);
        self.act_on_editor_outcome(tab_id, outcome);
    }

    /// Click the confirmation dialog button at index `idx` (e.g. 0 for Yes, 1 for No).
    pub(in crate::window) fn click_dialog_button(&mut self, idx: usize) {
        match &self.parts.overlay {
            Some(Overlay::Error(dialog)) => {
                if let Some(button) = dialog.buttons.buttons().get(idx) {
                    self.choose_on_error_dialog(button.value);
                }
            }
            Some(Overlay::Confirm(_)) => self.press_confirm_button(idx),
            Some(Overlay::EditorPrompt(prompt)) => {
                // A query replace's buttons each answer as its key does
                // (`y`, `n`, `!`, `q`).
                let answer = match &prompt.query_buttons {
                    Some(buttons) => match buttons.buttons().get(idx) {
                        Some(btn) => Some(btn.value.clone()),
                        None => return,
                    },
                    None => return,
                };
                self.term.search.match_index = 0;
                self.term.search.match_total = 0;
                self.term.search.current = None;
                self.host.prompts.debounce = None;
                let Some(Overlay::EditorPrompt(prompt)) = self
                    .parts
                    .overlay
                    .take_if(|o| matches!(o, Overlay::EditorPrompt(_)))
                else {
                    return;
                };
                self.dirty = true;
                let tab_id = prompt.tab;
                let reply = PromptReply {
                    answer,
                    tag: prompt.request.tag,
                };
                let Some(slot) = self.open_tabs.page_mut(&tab_id) else {
                    return;
                };
                let outcome = slot.page.on_prompt(reply);
                self.act_on_editor_outcome(tab_id, outcome);
            }
            _ => {}
        }
    }

    /// Hand an editor the choice made from the list it asked for, as the answer
    /// to the question it asked, closing the list. The window loop answers
    /// through [`Self::answer_pick`] from the palette it has taken out. A list closed without
    /// choosing answers with nothing, the way an escaped prompt does.
    #[cfg(test)]
    pub(in crate::window) fn answer_editor_pick(&mut self, choice: Option<String>) {
        let Some(mut overlay) = self.parts.take_picker_overlay() else {
            return;
        };
        match overlay.pick.take() {
            Some(pick) => self.answer_pick(pick, choice),
            // Not a pick: the picker was the window's own, and stays.
            None => self.parts.overlay = Some(Overlay::Picker(Box::new(overlay))),
        }
    }

    /// Hand the editor that asked `pick` the `choice` made for it.
    pub(in crate::window) fn answer_pick(&mut self, pick: ActivePick, choice: Option<String>) {
        let Some(slot) = self.open_tabs.page_mut(&pick.tab) else {
            return;
        };
        let outcome = slot.page.on_prompt(PromptReply {
            answer: choice,
            tag: pick.tag,
        });
        self.act_on_editor_outcome(pick.tab, outcome);
    }

    /// The dialog for the question that is open, if one is: an editor's, the
    /// host's own before a close, or the name of a theme being saved. A
    /// question is asked in the middle of the window, where the eye already
    /// is, and never on the status bar.
    pub(in crate::window) fn input_view(&self) -> Option<InputView> {
        // A search is typed in the find popup instead (see
        // [`App::find_target`]).
        if let Some(prompt) = self
            .parts
            .editor_prompt()
            .filter(|prompt| prompt.typed_search().is_none())
        {
            let buttons = prompt
                .query_buttons
                .as_ref()
                .map(|b| b.to_view())
                .unwrap_or_default();
            let mut view = input_dialog(
                &prompt.request.label,
                &prompt.field.text(),
                prompt.field.col(),
                prompt.request.mode,
                buttons,
            );
            view.card = prompt.origin;
            return Some(view);
        }
        if let Some(dialog) = self.parts.error_dialog() {
            return Some(InputView {
                buttons: dialog.buttons.to_view(),
                caret_visible: true,
                keys: ERROR_KEYS
                    .iter()
                    .map(|(key, does)| (key.to_string(), does.to_string()))
                    .collect(),
                kind: Some(crate::render::NoticeKind::Error),
                label: dialog.message(),
                title: Some(dialog.shown_title()),
                ..InputView::default()
            });
        }
        if let Some(view) = self.confirm_view() {
            return Some(view);
        }
        if let Some(input) = self.parts.password_input() {
            return Some(input_dialog(
                super::remote::PASSWORD_LABEL,
                &input.field.text(),
                input.field.col(),
                PromptMode::Secret,
                Vec::new(),
            ));
        }
        if let Some(input) = self.parts.secret_input() {
            return Some(input_dialog(
                &input.label,
                &input.field.text(),
                input.field.col(),
                PromptMode::Secret,
                Vec::new(),
            ));
        }
        let field = self.parts.theme_name_input()?;
        Some(input_dialog(
            THEME_NAME_LABEL,
            &field.text(),
            field.col(),
            PromptMode::Text,
            Vec::new(),
        ))
    }

    /// Take on the search an editor just ran as the window's own, the way a
    /// terminal's `/` is: its matches stay highlighted and counted once the
    /// question is answered, `Esc` puts them away, and the query is the one
    /// `n` and `N` repeat in a terminal too.
    pub(super) fn adopt_editor_search(&mut self, report: SearchReport) {
        self.term.search.match_index = report.index;
        self.term.search.match_total = report.total;
        self.term.search.current = None;
        self.term.search.origin = None;
        self.dirty = true;
        // While the question is open, what is typed into it is what is
        // highlighted: a search only becomes the window's once answered.
        if self.parts.editor_prompt().is_some() {
            return;
        }
        let was_hidden = self.term.search.query.is_none();
        self.term.search.last = Some(report.query.clone());
        self.term.search.query = Some(report.query);
        self.term.search.reverse = report.reverse;
        // Mirrors `start_search`: a live search forces a hidden status bar on.
        if was_hidden && !self.config.status_bar.enabled {
            self.resize_all_panes();
        }
    }
}

// ========================================================================
// EditorHost
// ========================================================================

impl EditorHost {
    /// Hand what has been typed into tab `tab`'s question to the editor once the
    /// typing pauses, so a search follows the typing without running on
    /// every key.
    pub(in crate::window) fn debounce_prompt_input(&mut self, tab: TabId) {
        self.prompts.debounce = Some((
            Instant::now() + Duration::from_millis(SEARCH_DEBOUNCE_MILLIS),
            tab,
        ));
    }
}
