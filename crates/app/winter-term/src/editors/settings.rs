//! The Settings page: every setting of Winter's and of its extensions, a
//! row each under its section, changed where it stands. Misty's
//! [`MistyPage`] draws it in the tab's pane and takes its keys and clicks
//! there (see `crate::window::frame::settings`): `j` and `k` move between
//! rows, `h` and `l` change a row's value, `i` or `RET` edit a text row
//! until `RET` or `Esc`, `TAB` moves between the sections and the rows,
//! `[` and `]` choose where edits go in a window with a workspace
//! (`settings.kdl` or the workspace), and `<backspace>` there drops a
//! workspace's own value of the row.
//!
//! The page keeps nothing of its own: each change is asked of the window
//! ([`EditorRequest::Setting`]), which puts it in force, keeps it, and hands
//! the page back the values in force (see [`SettingsEditor::refresh`]).

use misty::theme::Theme;
use misty::{
    CommandRegistry, EventCx, Frame as MistyFrame, Input, NumberRange, Rect, Setting, SettingValue,
    SettingsPage as MistyPage, SettingsResponse, SettingsSection, Widget,
};

use winter_model::editor::{EditorOutcome, EditorRequest, EditorStatus, Page, SettingEdit};
use winter_model::input::Key as WinterKey;
use winter_model::paint::{Frame, Row};
use winter_model::settings_page::{Control, Scope, SettingsField, SettingsPage, TextKind};

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

// ========================================================================
// Data Structures
// ========================================================================

/// The Settings page.
pub struct SettingsEditor {
    /// Winter's rows, which say what each of misty's rows sets and how its
    /// value is written.
    fields: SettingsPage,
    /// Misty's page, drawn in the tab's pane.
    page: MistyPage,
    /// The commands the page's keybindings section would rebind: none, as
    /// Winter's keys have a page of their own.
    registry: CommandRegistry,
    /// What the page has asked of the window, not yet taken.
    requests: Vec<EditorRequest>,
}

// ========================================================================
// SettingsEditor
// ========================================================================

impl SettingsEditor {
    /// The page over `fields`' rows.
    pub fn new(fields: SettingsPage) -> Self {
        let page = page_of(&fields);
        Self {
            fields,
            page,
            // An empty table is a valid one.
            registry: CommandRegistry::new(&[]).unwrap_or_else(|_| unreachable!()),
            requests: Vec::new(),
        }
    }

    /// Show the values now in force, from `fresh`, on the row and in the
    /// scope the page is on. Rows that came or went, or a workspace that
    /// came or went, build the page anew.
    pub fn refresh(&mut self, fresh: SettingsPage) {
        let same_rows = fresh.fields.len() == self.fields.fields.len()
            && fresh
                .fields
                .iter()
                .zip(&self.fields.fields)
                .all(|(new, old)| new.key == old.key && same_kind(&new.control, &old.control));
        let same_scopes = fresh.workspace.is_some() == self.fields.workspace.is_some();
        let scope = match fresh.workspace {
            Some(_) => self.fields.scope,
            None => Scope::User,
        };
        if same_rows && same_scopes {
            for field in &fresh.fields {
                self.page.set_value(&field.key, value_of(field));
            }
        } else {
            self.page = page_of(&fresh);
        }
        for field in &fresh.fields {
            self.page
                .set_overridden(&field.key, fresh.overridden.contains(&field.key));
        }
        self.fields = fresh;
        self.fields.scope = scope;
        self.page.set_scope(scope_index(scope));
    }

    /// Hand `input` to the page; whether it took it. A changed row asks the
    /// window to keep its value in the page's scope.
    pub fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> bool {
        let response = self.page.lend(&mut self.registry).handle(input, cx);
        match response {
            SettingsResponse::Changed(key) => {
                if let Some(value) = self.written(&key) {
                    self.requests.push(EditorRequest::Setting(SettingEdit {
                        key,
                        scope: self.fields.scope,
                        value,
                    }));
                }
                true
            }
            SettingsResponse::Cleared(key) => {
                self.requests.push(EditorRequest::ResetSetting(key));
                true
            }
            SettingsResponse::ScopeChanged(index) => {
                self.fields.scope = match index {
                    0 => Scope::User,
                    _ => Scope::Workspace,
                };
                true
            }
            // The page is a tab: Esc on it, or a press off it, leaves it
            // where it is.
            SettingsResponse::Dismissed => false,
            SettingsResponse::Handled | SettingsResponse::KeysChanged(_) => true,
            SettingsResponse::Ignored => false,
        }
    }

    /// Draw the page over `area` of `frame`.
    pub fn draw(&mut self, frame: &mut MistyFrame, area: Rect, theme: &Theme) {
        self.page.lend(&mut self.registry).draw(frame, area, theme);
    }

    /// The value of the row `key` as `settings.kdl` writes it, read off
    /// misty's row into a copy of Winter's.
    fn written(&self, key: &str) -> Option<String> {
        let mut field = self
            .fields
            .fields
            .iter()
            .find(|field| field.key == key)?
            .clone();
        let value = self.page.value(key)?;
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

    fn status(&self) -> EditorStatus {
        let scope = match self.fields.scope {
            Scope::User => USER_SCOPE,
            Scope::Workspace => WORKSPACE_SCOPE,
        };
        EditorStatus {
            mode: None,
            segments: vec![scope.to_string()],
        }
    }

    fn take_requests(&mut self) -> Vec<EditorRequest> {
        std::mem::take(&mut self.requests)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Misty's page for Winter's `fields`: a section for each heading, holding
/// every row under it (rows join a heading from more than one place), with
/// scope tabs when the window has a workspace.
fn page_of(fields: &SettingsPage) -> MistyPage {
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
    let page = sections
        .into_iter()
        .map(|(title, rows)| {
            rows.into_iter()
                .fold(SettingsSection::new(title), SettingsSection::with)
        })
        .fold(MistyPage::new(TITLE), MistyPage::with_section)
        .with_edits_kept_on_escape();
    let mut page = match &fields.workspace {
        Some(name) => page.with_scopes(vec![USER_SCOPE.to_string(), name.clone()]),
        None => page,
    };
    page.set_scope(scope_index(fields.scope));
    for key in &fields.overridden {
        page.set_overridden(key, true);
    }
    page
}

/// Misty's row for Winter's `field`. A color is a text row: an empty one
/// leaves the theme's own, and one may carry an alpha.
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

/// Misty's value for Winter's `field`.
fn value_of(field: &SettingsField) -> SettingValue {
    match &field.control {
        Control::Choice(choice) => SettingValue::Choice(choice.index),
        Control::Number(number) => SettingValue::Number(f64::from(number.value)),
        Control::Text(text) => match text.kind {
            TextKind::Names(_) => {
                SettingValue::Names(text.value.split_whitespace().map(str::to_owned).collect())
            }
            TextKind::Color | TextKind::Plain => SettingValue::Text(text.value.clone()),
        },
        Control::Toggle(toggle) => SettingValue::Toggle(toggle.on),
    }
}

/// Whether `a` and `b` are rows of the same kind, so misty's row for one
/// takes the other's value.
fn same_kind(a: &Control, b: &Control) -> bool {
    match (a, b) {
        (Control::Choice(a), Control::Choice(b)) => a.options == b.options,
        (Control::Text(a), Control::Text(b)) => a.kind == b.kind,
        (Control::Number(_), Control::Number(_)) | (Control::Toggle(_), Control::Toggle(_)) => true,
        _ => false,
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
    use misty::key::{Key, KeyCode, Mods};
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

    /// Hand `editor` a press of each of `codes`.
    fn press(editor: &mut SettingsEditor, codes: &[KeyCode]) {
        let mut window = TestWindow::new(800, 600);
        for code in codes {
            let key = Key::new(*code, Mods::NONE);
            editor.handle(&Input::Key(key), &mut window.cx());
        }
    }

    #[test]
    fn test_changing_a_row_asks_the_window_to_keep_it_in_the_page_s_scope() {
        let mut editor = page();
        press(&mut editor, &[KeyCode::Char('l')]);
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
    fn test_a_names_row_is_written_as_words_once_its_editing_ends() {
        let mut editor = page();
        // The names row heads no section, so it follows Shell in Terminal.
        press(
            &mut editor,
            &[KeyCode::Tab, KeyCode::Char('j'), KeyCode::Tab],
        );
        press(&mut editor, &[KeyCode::Char('j'), KeyCode::Char('i')]);
        press(
            &mut editor,
            &[KeyCode::End, KeyCode::Char(' '), KeyCode::Char('c')],
        );
        assert!(
            editor.take_requests().is_empty(),
            "nothing kept while typing"
        );
        press(&mut editor, &[KeyCode::Esc]);
        assert_eq!(
            editor.take_requests(),
            [EditorRequest::Setting(SettingEdit {
                key: "status.segments".to_string(),
                scope: Scope::User,
                value: "git c".to_string(),
            })]
        );
    }

    #[test]
    fn test_refreshing_keeps_the_page_and_drops_a_scope_no_longer_there() {
        let mut editor = page();
        let mut fresh = page().fields;
        fresh.workspace = Some("work".to_string());
        editor.refresh(fresh);
        press(&mut editor, &[KeyCode::Char(']')]);
        assert_eq!(editor.fields.scope, Scope::Workspace);
        editor.refresh(page().fields);
        assert_eq!(editor.fields.scope, Scope::User, "no workspace, no scope");
        if let Control::Toggle(toggle) = &mut editor.fields.fields[0].control {
            toggle.on = true;
        }
        let mut on = editor.fields.clone();
        on.fields[0].control =
            Control::Toggle(winter_model::settings_page::ToggleControl { on: true });
        editor.refresh(on);
        assert_eq!(
            editor.page.value("ligatures"),
            Some(&SettingValue::Toggle(true))
        );
    }
}
