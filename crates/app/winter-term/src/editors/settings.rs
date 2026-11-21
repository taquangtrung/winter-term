//! The Settings page: every setting of Winter's and of its extensions, a
//! row each under its section, changed where it stands. It is misty's
//! settings page ([`settings_page`]), a tree of the page kit's elements
//! drawn in the tab's pane, held in a [`ViewHost`].
//!
//! The page has Winter's explicit modes. In Normal mode `j` and `k` move
//! between the controls, `h` and `l` change the one focused, `/` finds a
//! setting by typing into the filter, `[` and `]` choose where edits go in
//! a window with a workspace (`settings.kdl` or the workspace), and `i` or
//! `RET` enter Insert mode, where every key goes to the focused control
//! (typing into a text row) until `Esc`. `TAB`, `SPC`, and the arrows reach
//! the controls in either mode.
//!
//! The page keeps nothing of its own: each change is asked of the window
//! ([`EditorRequest::Setting`]), which puts it in force, keeps it, and hands
//! the page back the values in force (see [`SettingsEditor::refresh`]).

use std::collections::BTreeSet;
use std::time::Instant;

use misty::view::{
    HostedView, SettingChange, SettingEdit as MistyEdit, SettingScope, SettingsPage as MistyPage,
    settings_page,
};
use misty::{
    AccessAction, Element, EventCx, Frame as MistyFrame, Input, Key, KeyCode, Mods, NumberRange,
    Rect, Setting, SettingValue, SettingsSection, Strings, Theme,
};

use winter_model::editor::{
    EditorOutcome, EditorRequest, EditorStatus, Page, PageView, SettingEdit,
};
use winter_model::input::Key as WinterKey;
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::modal::EditMode;
use winter_model::paint::{Frame, Row};
use winter_model::settings_page::{Control, Scope, SettingsField, SettingsPage, TextKind};

use crate::window::winter_key;

// ========================================================================
// Constants
// ========================================================================

/// What the page and its tab are called.
const TITLE: &str = "Settings";

/// The scope tabs' names.
const USER_SCOPE: &str = "User";
const WORKSPACE_SCOPE: &str = "Workspace";

/// The section rows before any named one go under.
const FIRST_SECTION: &str = "General";

/// The page's keys in Normal mode, block `settings`.
const KEYS: KeyTable<Act> = KeyTable {
    block: "settings",
    bindings: &[
        Binding {
            action: Act::Next,
            id: "next_control",
            keys: &["j"],
            label: "Next Control",
        },
        Binding {
            action: Act::Previous,
            id: "previous_control",
            keys: &["k"],
            label: "Previous Control",
        },
        Binding {
            action: Act::Decrease,
            id: "decrease",
            keys: &["h"],
            label: "Change the Setting Back",
        },
        Binding {
            action: Act::Increase,
            id: "increase",
            keys: &["l"],
            label: "Change the Setting On",
        },
        Binding {
            action: Act::Find,
            id: "find_setting",
            keys: &["/"],
            label: "Find a Setting",
        },
        Binding {
            action: Act::UserScope,
            id: "user_scope",
            keys: &["["],
            label: "Edit Your Settings",
        },
        Binding {
            action: Act::WorkspaceScope,
            id: "workspace_scope",
            keys: &["]"],
            label: "Edit the Workspace's Settings",
        },
        Binding {
            action: Act::Insert,
            id: "insert",
            keys: &["i"],
            label: "Type into the Control",
        },
        Binding {
            action: Act::Activate,
            id: "activate",
            keys: &["RET"],
            label: "Press or Edit the Control",
        },
    ],
    configurable: true,
    title: "Settings",
};

/// The page's keys in Insert mode, block `settings.insert`.
const INSERT_KEYS: KeyTable<Act> = KeyTable {
    block: "settings.insert",
    bindings: &[Binding {
        action: Act::Leave,
        id: "leave_insert",
        keys: &["ESC"],
        label: "Back to Normal Mode",
    }],
    configurable: true,
    title: "Settings: Insert",
};

/// The page's key tables, for the Keys page and `keybindings.kdl`.
pub(crate) const KEY_TABLES: &[&dyn Listed] = &[&KEYS, &INSERT_KEYS];

// ========================================================================
// Data Structures
// ========================================================================

/// What a Settings key does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    /// Press the focused control, then type into it.
    Activate,
    /// Change the focused control's setting back, as Left does.
    Decrease,
    /// Type into the filter.
    Find,
    /// Change the focused control's setting on, as Right does.
    Increase,
    /// Type into the focused control.
    Insert,
    /// Leave Insert mode.
    Leave,
    /// Focus the next control.
    Next,
    /// Focus the previous control.
    Previous,
    /// Send edits to the user's settings.
    UserScope,
    /// Send edits to the workspace's settings.
    WorkspaceScope,
}

/// The Settings page.
pub struct SettingsEditor {
    /// Whether keys go to the focused control (Insert) or move about it
    /// (Normal).
    mode: EditMode,
    /// The page's tree and what it is built from.
    view: PageView<SettingsState>,
}

/// What the page's tree is built from, and what it asks of the window.
struct SettingsState {
    /// Winter's rows, which say what each of misty's rows sets and how its
    /// value is written.
    fields: SettingsPage,
    /// The text the rows are kept to.
    filter: String,
    /// What the page has asked of the window, not yet taken.
    requests: Vec<EditorRequest>,
    /// The scope tabs' names: the user's, then the workspace's, if any.
    scope_names: Vec<String>,
    /// Misty's sections, built from the rows with the values in force.
    sections: Vec<SettingsSection>,
    /// The section shown.
    shown: usize,
    /// The theme's words, as the last frame was drawn in.
    strings: Strings,
}

// ========================================================================
// SettingsEditor
// ========================================================================

impl SettingsEditor {
    /// The page over `fields`' rows.
    pub fn new(fields: SettingsPage) -> Self {
        let mut state = SettingsState {
            fields: SettingsPage::new(Vec::new()),
            filter: String::new(),
            requests: Vec::new(),
            scope_names: Vec::new(),
            sections: Vec::new(),
            shown: 0,
            strings: Strings::default(),
        };
        state.take_fields(fields);
        Self {
            mode: EditMode::Normal,
            view: PageView::with_keys(state),
        }
    }

    /// Show the values now in force, from `fresh`, in the scope the page is
    /// on; a workspace that went takes its scope with it.
    pub fn refresh(&mut self, fresh: SettingsPage) {
        let state = self.view.state_mut();
        let scope = match fresh.workspace {
            Some(_) => state.fields.scope,
            None => Scope::User,
        };
        state.take_fields(fresh);
        state.fields.scope = scope;
    }

    /// Hand `input` to the page; whether it took it. A key of the mode's
    /// table is the page's; in Normal mode a letter no table binds is
    /// dropped, as it would type into a text row; the rest go to the
    /// controls.
    pub fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> bool {
        let key = match input {
            Input::Key(key) => winter_key(*key),
            _ => None,
        };
        let table = match self.mode {
            EditMode::Insert => &INSERT_KEYS,
            _ => &KEYS,
        };
        if let Some(act) = key.as_ref().and_then(|key| table::find(table, key)) {
            self.run(act, input, cx);
            cx.request_redraw();
            return true;
        }
        let typing = key
            .and_then(|key| key.typed_char())
            .is_some_and(|c| c != ' ');
        if self.mode != EditMode::Insert && typing {
            return true;
        }
        self.view.view_input(input, cx)
    }

    /// Do what `act` asks, `input` being the key that asked it.
    fn run(&mut self, act: Act, input: &Input, cx: &mut EventCx<'_>) {
        let press = |code| Input::Key(Key::new(code, Mods::NONE));
        match act {
            Act::Next => self.view.move_focus(1),
            Act::Previous => self.view.move_focus(-1),
            Act::Decrease => {
                self.view.view_input(&press(KeyCode::Left), cx);
            }
            Act::Increase => {
                self.view.view_input(&press(KeyCode::Right), cx);
            }
            Act::Find => {
                self.view.focus(MistyPage::<SettingsState>::FILTER_ID);
                self.mode = EditMode::Insert;
            }
            Act::UserScope => self.view.state_mut().choose_scope(Scope::User),
            Act::WorkspaceScope => self.view.state_mut().choose_scope(Scope::Workspace),
            Act::Insert => self.mode = EditMode::Insert,
            Act::Activate => {
                self.view.view_input(input, cx);
                self.mode = EditMode::Insert;
            }
            // Esc still reaches the control: it puts a half-typed value back.
            Act::Leave => {
                self.view.view_input(input, cx);
                self.mode = EditMode::Normal;
            }
        }
    }

    /// Draw the page over `area` of `frame`, in `theme`'s words.
    pub fn draw(&mut self, frame: &mut MistyFrame, area: Rect, theme: &Theme) {
        if self.view.state().strings != theme.strings {
            self.view.state_mut().strings = theme.strings.clone();
        }
        self.view.draw_view(frame, area, theme);
    }
}

impl Page for SettingsEditor {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    /// Misty draws the page; the grid under it is empty.
    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![Row::new()])
    }

    /// Misty's page takes the keys before Winter's dispatch reaches here.
    fn on_key(&mut self, _key: &WinterKey) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    fn is_view(&self) -> bool {
        true
    }

    fn draw_view(&mut self, frame: &mut MistyFrame, area: Rect, theme: &Theme) {
        self.draw(frame, area, theme);
    }

    fn view_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Option<EditorOutcome> {
        self.handle(input, cx).then_some(EditorOutcome::Consumed)
    }

    fn view_access_action(&mut self, action: AccessAction) -> Option<EditorOutcome> {
        self.view
            .access_action(action)
            .then_some(EditorOutcome::Consumed)
    }

    fn view_about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.view.view_about_to_wait(cx)
    }

    fn status(&self) -> EditorStatus {
        let scope = match self.view.state().fields.scope {
            Scope::User => USER_SCOPE,
            Scope::Workspace => WORKSPACE_SCOPE,
        };
        EditorStatus {
            mode: Some(self.mode),
            segments: vec![scope.to_string()],
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut keys = table::help(&KEYS);
        keys.extend(table::help(&INSERT_KEYS));
        keys
    }

    fn take_requests(&mut self) -> Vec<EditorRequest> {
        std::mem::take(&mut self.view.state_mut().requests)
    }
}

// ========================================================================
// SettingsState
// ========================================================================

impl SettingsState {
    /// Take `fields`' rows and values, and the scopes its workspace makes.
    fn take_fields(&mut self, fields: SettingsPage) {
        self.sections = sections_of(&fields);
        self.shown = self.shown.min(self.sections.len().saturating_sub(1));
        self.scope_names = match &fields.workspace {
            Some(name) => vec![USER_SCOPE.to_string(), name.clone()],
            None => Vec::new(),
        };
        self.fields = fields;
    }

    /// Send edits to `scope`, when the window has a workspace to send them
    /// to.
    fn choose_scope(&mut self, scope: Scope) {
        if self.fields.workspace.is_some() {
            self.fields.scope = scope;
        }
    }

    /// Queue what `change` asks of the window: a value to keep in the
    /// page's scope, or the workspace's own value to drop.
    fn answer(&mut self, change: SettingChange) {
        match change.edit {
            MistyEdit::Changed(value) => {
                // Shown at once, before the window hands the values back.
                for section in &mut self.sections {
                    section.set_value(&change.id, value.clone());
                }
                if let Some(value) = self.written(&change.id, &value) {
                    self.requests.push(EditorRequest::Setting(SettingEdit {
                        key: change.id,
                        scope: self.fields.scope,
                        value,
                    }));
                }
            }
            MistyEdit::Cleared => self.requests.push(EditorRequest::ResetSetting(change.id)),
            // Winter's rows hold values; none is an action. A kind misty
            // adds later is none of Winter's either.
            _ => {}
        }
    }

    /// `value`, of the row `key`, as `settings.kdl` writes it, read into a
    /// copy of Winter's row.
    fn written(&self, key: &str, value: &SettingValue) -> Option<String> {
        let mut field = self
            .fields
            .fields
            .iter()
            .find(|field| field.key == key)?
            .clone();
        match (&mut field.control, value) {
            (Control::Choice(choice), SettingValue::Choice(index)) => choice.index = *index,
            (Control::Number(number), SettingValue::Number(value)) => number.value = *value as f32,
            (Control::Text(text), SettingValue::Names(names)) => text.value = names.join(" "),
            (Control::Text(text), SettingValue::Text(value)) => text.value = value.clone(),
            (Control::Toggle(toggle), SettingValue::Toggle(on)) => toggle.on = *on,
            _ => return None,
        }
        Some(field.control.value())
    }

    /// Where the setting `id` stands against the scopes: one the
    /// workspace sets is overridden in the user's scope, and the
    /// workspace's own in its.
    fn scope_of(overridden: &BTreeSet<String>, scope: Scope, id: &str) -> SettingScope {
        match (overridden.contains(id), scope) {
            (false, _) => SettingScope::InForce,
            (true, Scope::User) => SettingScope::Overridden,
            (true, Scope::Workspace) => SettingScope::OwnValue,
        }
    }
}

impl HostedView for SettingsState {
    fn view(&self) -> Element<'_, Self> {
        let page = settings_page(TITLE, &self.sections, &self.strings)
            .shown(self.shown, |state: &mut Self, at| state.shown = at)
            .filter(&self.filter, |state: &mut Self, filter| {
                state.filter = filter
            })
            .on_change(|state: &mut Self, change| state.answer(change));
        if self.scope_names.is_empty() {
            return page.into();
        }
        let (overridden, scope) = (&self.fields.overridden, self.fields.scope);
        page.scopes(
            &self.scope_names,
            scope_index(scope),
            |state: &mut Self, at| {
                state.choose_scope(match at {
                    0 => Scope::User,
                    _ => Scope::Workspace,
                });
            },
            move |id| Self::scope_of(overridden, scope, id),
        )
        .into()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Misty's sections for Winter's `fields`: one for each heading, holding
/// every row under it (rows join a heading from more than one place).
fn sections_of(fields: &SettingsPage) -> Vec<SettingsSection> {
    let mut sections: Vec<(String, Vec<Setting>)> = Vec::new();
    let mut current = FIRST_SECTION.to_string();
    for field in &fields.fields {
        if let Some(title) = &field.section {
            current = title.clone();
        }
        match sections.iter_mut().find(|(title, _)| *title == current) {
            Some((_, rows)) => rows.push(setting_of(field)),
            None => sections.push((current.clone(), vec![setting_of(field)])),
        }
    }
    sections
        .into_iter()
        .map(|(title, rows)| {
            rows.into_iter()
                .fold(SettingsSection::new(title), SettingsSection::with)
        })
        .collect()
}

/// Misty's row for Winter's `field`, holding its value in force. A color
/// is a text row: an empty one leaves the theme's own, and one may carry
/// an alpha.
fn setting_of(field: &SettingsField) -> Setting {
    let (key, label) = (field.key.as_str(), field.label.as_str());
    let setting = match &field.control {
        Control::Choice(choice) => {
            let options: Vec<&str> = choice
                .options
                .iter()
                .map(|option| option.label.as_str())
                .collect();
            Setting::choice(key, label, &options, choice.index)
        }
        Control::Number(number) => {
            let range = NumberRange::new(
                f64::from(number.min),
                f64::from(number.max),
                f64::from(number.step),
            );
            Setting::number(key, label, f64::from(number.value), range)
        }
        Control::Text(text) => match &text.kind {
            TextKind::Names(choices) => {
                let held = text.value.split_whitespace().map(str::to_owned).collect();
                Setting::names(key, label, held, choices.clone())
            }
            TextKind::Color | TextKind::Plain => Setting::text(key, label, text.value.as_str()),
        },
        Control::Toggle(toggle) => Setting::toggle(key, label, toggle.on),
    };
    match &field.note {
        Some(note) => setting.with_description(note.as_str()),
        None => setting,
    }
}

/// The index of `scope` among the page's scope tabs.
fn scope_index(scope: Scope) -> usize {
    match scope {
        Scope::User => 0,
        Scope::Workspace => 1,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use misty::testing::TestWindow;

    use super::*;

    fn page() -> SettingsEditor {
        SettingsEditor::new(SettingsPage::new(vec![
            SettingsField::toggle("ligatures", "Ligatures", false).in_section("Font"),
            SettingsField::text("shell", "Shell", String::new()).in_section("Terminal"),
            SettingsField::names(
                "status.segments",
                "Segments",
                &["git".to_string()],
                vec!["git".to_string(), "clock".to_string()],
            ),
        ]))
    }

    /// Hand `editor` a press of each of `codes`, drawing it first so its
    /// controls are laid out.
    fn press(editor: &mut SettingsEditor, codes: &[KeyCode]) {
        let mut window = TestWindow::new(1200, 800);
        for code in codes {
            crate::editors::view_tests::drawn(editor, &window);
            let key = Key::new(*code, Mods::NONE);
            editor.handle(&Input::Key(key), &mut window.cx());
        }
        crate::editors::view_tests::drawn(editor, &window);
    }

    /// The keys that type `text`.
    fn typing(text: &str) -> Vec<KeyCode> {
        text.chars().map(KeyCode::Char).collect()
    }

    fn shown(editor: &mut SettingsEditor) -> String {
        let window = TestWindow::new(1200, 800);
        crate::editors::view_tests::text_of(&crate::editors::view_tests::drawn(editor, &window))
    }

    #[test]
    fn test_slash_finds_a_setting_in_another_section_by_what_is_typed() {
        let mut editor = page();
        let codes: Vec<KeyCode> = [KeyCode::Char('/')]
            .into_iter()
            .chain(typing("shell"))
            .collect();
        press(&mut editor, &codes);
        assert_eq!(editor.mode, EditMode::Insert, "typing into the filter");
        let shown = shown(&mut editor);
        assert!(
            shown.contains("Shell") && !shown.contains("Ligatures"),
            "{shown:?}"
        );
        press(&mut editor, &[KeyCode::Esc]);
        assert_eq!(editor.mode, EditMode::Normal);
    }

    #[test]
    fn test_changing_a_row_asks_the_window_to_keep_it_in_the_page_s_scope() {
        let mut editor = page();
        editor.view.focus("ligatures");
        press(&mut editor, &[KeyCode::Char(' ')]);
        assert_eq!(
            editor.take_requests(),
            [EditorRequest::Setting(SettingEdit {
                key: "ligatures".to_string(),
                scope: Scope::User,
                value: "true".to_string(),
            })]
        );
        assert!(editor.take_requests().is_empty(), "asked once");
    }

    #[test]
    fn test_a_letter_in_normal_mode_never_types_into_a_text_row() {
        let mut editor = page();
        editor.view.state_mut().shown = 1;
        editor.view.focus("shell");
        press(&mut editor, &typing("ab"));
        assert!(
            editor.take_requests().is_empty(),
            "nothing typed in Normal mode"
        );
        press(&mut editor, &[KeyCode::Char('i')]);
        press(&mut editor, &typing("sh"));
        let asked = editor.take_requests();
        assert!(
            matches!(asked.last(), Some(EditorRequest::Setting(edit)) if edit.key == "shell" && edit.value == "sh"),
            "{asked:?}"
        );
    }

    #[test]
    fn test_refreshing_keeps_the_page_and_drops_a_scope_no_longer_there() {
        let mut editor = page();
        let mut fresh = page().view.state().fields.clone();
        fresh.workspace = Some("work".to_string());
        editor.refresh(fresh);
        press(&mut editor, &[KeyCode::Char(']')]);
        assert_eq!(editor.view.state().fields.scope, Scope::Workspace);
        assert!(shown(&mut editor).contains("work"), "the workspace's tab");
        editor.refresh(page().view.state().fields.clone());
        assert_eq!(
            editor.view.state().fields.scope,
            Scope::User,
            "no workspace, no scope"
        );
    }

    #[test]
    fn test_a_screen_reader_s_click_on_a_toggle_queues_the_setting() {
        let mut editor = page();
        let frame = crate::editors::view_tests::drawn(&mut editor, &TestWindow::new(1200, 800));
        let toggle = misty::testing::node_named(&frame, "Ligatures").expect("described");
        let used = editor.view_access_action(AccessAction::Click(toggle));
        assert_eq!(used, Some(EditorOutcome::Consumed));
        assert!(
            matches!(editor.view.state().requests.last(), Some(EditorRequest::Setting(edit)) if edit.key == "ligatures"),
            "{:?}",
            editor.view.state().requests
        );
    }

    #[test]
    fn test_every_control_says_what_it_is() {
        let mut editor = page();
        let frame = crate::editors::view_tests::drawn(&mut editor, &TestWindow::new(1200, 800));
        let unnamed = misty::testing::unnamed_controls(&frame);
        assert!(unnamed.is_empty(), "{unnamed:?}");
    }
}
