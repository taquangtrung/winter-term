//! The Settings page: every setting of Winter's and of its extensions, a
//! row each under its section, changed where it stands. In Normal, `j` and
//! `k` move between rows, `h` and `l` (or `SPC`) change the row's value, and
//! `i` edits a text row, in Insert until `Esc`. `TAB` chooses where edits go
//! in a window with a workspace, `settings.kdl` or the workspace, and
//! `S-<delete>` drops a workspace's own value of the row.
//!
//! The page keeps nothing of its own: each change is asked of the window
//! ([`EditorRequest::Setting`]), which puts it in force, keeps it, and hands
//! the page back the values in force (see [`SettingsEditor::refresh`]).

use winter_model::editor::{EditorOutcome, EditorRequest, EditorStatus, Page, SettingEdit};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::input::{Key, KeyCode, Mods};
use winter_model::paint::{Frame, Row, Span, Style};
use winter_model::settings_page::{Control, Scope, SettingsField, SettingsPage};
use winter_model::textedit::modal::EditMode;

// ========================================================================
// Constants
// ========================================================================

/// What the page and its tab are called.
const TITLE: &str = "Settings";

/// The page's layout, in cells: where labels start, where notes start, and
/// the margin values are aligned to.
const LEFT_PAD: usize = 4;
const NOTE_COL: usize = 28;
const RIGHT_PAD: usize = 4;

/// What marks a row the workspace sets over `settings.kdl`, and its column.
const OVERRIDDEN_MARK: &str = "●";
const OVERRIDDEN_COL: usize = 2;

/// The columns between the title and the scope tabs, and between the tabs.
const SCOPE_GAP: usize = 4;

/// The scope tabs' names.
const USER_SCOPE: &str = "User";
const WORKSPACE_SCOPE: &str = "Workspace";

/// What stands at the caret of a text row being edited.
const TEXT_CARET: &str = "\u{258f}";

/// What an empty text row shows, which leaves the default in force.
const DEFAULT_TEXT: &str = "default";

/// The rows above the settings: the title, the keys, and a blank.
const HEADER_ROWS: usize = 3;

/// The keys said under the title, by what the page is doing.
const HINT: &str = "j/k Move   h/l Change   SPC Toggle   i Edit Text";
const HINT_SCOPED: &str = "j/k Move   h/l Change   SPC Toggle   i Edit Text   TAB Scope";
const HINT_WORKSPACE: &str =
    "j/k Move   h/l Change   i Edit Text   TAB Scope   S-<delete> Use User's";
const HINT_INSERT: &str = "Typing into the setting   Esc Done";

/// The page's keys, block `settings`.
const KEYS: KeyTable<Act> = KeyTable {
    block: "settings",
    bindings: &[
        Binding {
            action: Act::Down,
            id: "next_setting",
            keys: &["j", "<down>"],
            label: "Next Setting",
        },
        Binding {
            action: Act::Up,
            id: "previous_setting",
            keys: &["k", "<up>"],
            label: "Previous Setting",
        },
        Binding {
            action: Act::Back,
            id: "change_back",
            keys: &["h", "<left>"],
            label: "Change the Setting Back a Step",
        },
        Binding {
            action: Act::Forward,
            id: "change_forward",
            keys: &["l", "<right>", "SPC"],
            label: "Change the Setting On a Step",
        },
        Binding {
            action: Act::Edit,
            id: "edit_setting",
            keys: &["i", "RET"],
            label: "Edit a Text Setting, or Change Another On",
        },
        Binding {
            action: Act::Scope,
            id: "switch_scope",
            keys: &["TAB"],
            label: "Switch Between User and Workspace Settings",
        },
        Binding {
            action: Act::Reset,
            id: "use_user_setting",
            keys: &["S-<delete>"],
            label: "Drop the Workspace's Value, Using the User's",
        },
    ],
    configurable: true,
    title: "Settings",
};

/// The page's key tables, for the Keys page and `keybindings.kdl`.
pub(crate) const KEY_TABLES: &[&dyn Listed] = &[&KEYS];

// ========================================================================
// Data Structures
// ========================================================================

/// What a settings key does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    Back,
    Down,
    Edit,
    Forward,
    Reset,
    Scope,
    Up,
}

/// The Settings page.
pub struct SettingsEditor {
    /// Whether a text row is being typed into: Insert, until `Esc`.
    inserting: bool,
    page: SettingsPage,
    /// What the page has asked of the window, not yet taken.
    requests: Vec<EditorRequest>,
    /// The first setting row shown, scrolled to keep the selected one in view.
    top: usize,
}

// ========================================================================
// SettingsEditor
// ========================================================================

impl SettingsEditor {
    /// The page over `page`'s rows.
    pub fn new(page: SettingsPage) -> Self {
        Self {
            inserting: false,
            page,
            requests: Vec::new(),
            top: 0,
        }
    }

    /// Show the values now in force, from `fresh`, on the row and in the
    /// scope the page is on.
    pub fn refresh(&mut self, fresh: SettingsPage) {
        let (selected, scope) = (self.page.selected, self.page.scope);
        self.page = fresh;
        self.page.selected = selected.min(self.page.fields.len().saturating_sub(1));
        self.page.scope = match self.page.workspace {
            Some(_) => scope,
            None => Scope::User,
        };
        self.inserting &= self.page.selected_is_text();
    }

    /// Ask the window to keep `edit`, a row's new value, in the page's scope.
    fn ask(&mut self, edit: Option<(String, String)>) {
        if let Some((key, value)) = edit {
            self.requests.push(EditorRequest::Setting(SettingEdit {
                key,
                scope: self.page.scope,
                value,
            }));
        }
    }

    fn run(&mut self, act: Act) -> EditorOutcome {
        match act {
            Act::Back => {
                let edit = self.page.adjust(false);
                self.ask(edit);
            }
            Act::Down => self.page.move_down(),
            Act::Edit if self.page.selected_is_text() => self.inserting = true,
            Act::Edit | Act::Forward => {
                let edit = self.page.adjust(true);
                self.ask(edit);
            }
            Act::Reset => {
                let key = self.page.selected_key().map(str::to_string);
                if let Some(key) = key.filter(|_| self.page.scope == Scope::Workspace) {
                    self.requests.push(EditorRequest::ResetSetting(key));
                }
            }
            Act::Scope => {
                self.page.toggle_scope();
            }
            Act::Up => self.page.move_up(),
        }
        EditorOutcome::Consumed
    }

    /// The title row: the page's name, then the scope tabs in a window
    /// with a workspace, the one edits go to in the accent color.
    fn title_row(&self) -> Row {
        let mut row = vec![
            Span::plain(" ".repeat(LEFT_PAD)),
            Span::new(Style::HeaderAccent, TITLE),
        ];
        let Some(name) = self.page.workspace.as_deref() else {
            return row;
        };
        let tab = |chosen: bool| match chosen {
            true => Style::HeaderAccent,
            false => Style::Dim,
        };
        let gap = " ".repeat(SCOPE_GAP);
        row.extend([
            Span::plain(gap.clone()),
            Span::new(tab(self.page.scope == Scope::User), USER_SCOPE),
            Span::plain(gap),
            Span::new(
                tab(self.page.scope == Scope::Workspace),
                format!("{WORKSPACE_SCOPE}: {name}"),
            ),
        ]);
        row
    }

    /// The keys said under the title.
    fn hint(&self) -> &'static str {
        match (self.inserting, &self.page.workspace, self.page.scope) {
            (true, _, _) => HINT_INSERT,
            (false, None, _) => HINT,
            (false, Some(_), Scope::User) => HINT_SCOPED,
            (false, Some(_), Scope::Workspace) => HINT_WORKSPACE,
        }
    }

    /// Every settings row, section headings among them, `cols` wide, and
    /// which of them is the selected setting's.
    fn body(&self, cols: usize) -> (Vec<Row>, usize) {
        let mut rows = Vec::new();
        let mut selected_row = 0;
        let mut section: Option<&str> = None;
        for (at, field) in self.page.fields.iter().enumerate() {
            if let Some(name) = field
                .section
                .as_deref()
                .filter(|name| section != Some(name))
            {
                section = Some(name);
                if !rows.is_empty() {
                    rows.push(Vec::new());
                }
                rows.push(vec![
                    Span::plain(" ".repeat(LEFT_PAD)),
                    Span::new(Style::HeaderAccent, name.to_uppercase()),
                ]);
            }
            let selected = at == self.page.selected;
            if selected {
                selected_row = rows.len();
            }
            let caret = (selected && self.inserting)
                .then(|| self.page.text_caret())
                .flatten();
            let overridden = self.page.overridden.contains(&field.key);
            rows.push(field_row(field, cols, caret, overridden));
        }
        (rows, selected_row)
    }
}

impl Page for SettingsEditor {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        let (body, selected) = self.body(cols);
        let shown = rows.saturating_sub(HEADER_ROWS).max(1);
        // Scrolled just far enough to keep the selected setting in view.
        self.top = self.top.min(selected);
        if selected >= self.top + shown {
            self.top = selected + 1 - shown;
        }
        let mut frame_rows = vec![
            self.title_row(),
            vec![
                Span::plain(" ".repeat(LEFT_PAD)),
                Span::new(Style::Dim, self.hint()),
            ],
            Vec::new(),
        ];
        frame_rows.extend(body.into_iter().skip(self.top).take(shown));
        Frame::new(frame_rows).with_cursor_line(HEADER_ROWS + selected - self.top)
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        if self.inserting {
            // key-table: Insert's own way back to Normal, as every editor's is.
            if key.chord() == (Mods::NONE, KeyCode::Escape) {
                self.inserting = false;
            } else {
                let edit = self.page.edit_text(key);
                self.ask(edit);
            }
            return EditorOutcome::Consumed;
        }
        match table::find(&KEYS, key) {
            Some(act) => self.run(act),
            None => EditorOutcome::Ignored,
        }
    }

    fn on_paste(&mut self, text: String) -> EditorOutcome {
        if self.inserting {
            let edit = self.page.paste_text(&text);
            self.ask(edit);
        }
        EditorOutcome::Consumed
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        for _ in 0..lines.unsigned_abs() {
            match lines > 0 {
                true => self.page.move_up(),
                false => self.page.move_down(),
            }
        }
        EditorOutcome::Consumed
    }

    fn keys(&self) -> Vec<KeyHelp> {
        table::help(&KEYS)
    }

    fn picks_rows(&self) -> bool {
        true
    }

    fn status(&self) -> EditorStatus {
        let scope = match self.page.scope {
            Scope::User => USER_SCOPE,
            Scope::Workspace => WORKSPACE_SCOPE,
        };
        EditorStatus {
            mode: Some(match self.inserting {
                true => EditMode::Insert,
                false => EditMode::Normal,
            }),
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

/// One setting's row, `cols` wide: the mark of a workspace's own value, the
/// label, the note, and the value aligned to the right margin, with the
/// caret at `caret` in a text row being edited.
fn field_row(field: &SettingsField, cols: usize, caret: Option<usize>, overridden: bool) -> Row {
    let value = value_spans(&field.control, caret);
    let value_width: usize = value.iter().map(|span| span.text.chars().count()).sum();
    let value_col = cols.saturating_sub(RIGHT_PAD + value_width);
    let mut row = vec![Span::plain(" ".repeat(OVERRIDDEN_COL))];
    let mark = match overridden {
        true => Span::new(Style::Accent, OVERRIDDEN_MARK),
        false => Span::plain(" "),
    };
    row.push(mark);
    row.push(Span::plain(" ".repeat(LEFT_PAD - OVERRIDDEN_COL - 1)));
    row.push(Span::new(Style::Normal, field.label.clone()));
    let mut col = LEFT_PAD + field.label.chars().count();
    if let Some(note) = field.note.as_deref().filter(|_| NOTE_COL + 1 < value_col) {
        let start = NOTE_COL.max(col + 1);
        let budget = value_col.saturating_sub(start + 1);
        row.push(Span::plain(" ".repeat(start - col)));
        let note: String = note.chars().take(budget).collect();
        col = start + note.chars().count();
        row.push(Span::new(Style::Dim, note));
    }
    row.push(Span::plain(
        " ".repeat(value_col.saturating_sub(col).max(1)),
    ));
    row.extend(value);
    row
}

/// A setting's value: a toggle's `ON` or `OFF`, a choice or number as
/// `‹ value ›`, and text as it is, the caret at `caret` while it is edited.
fn value_spans(control: &Control, caret: Option<usize>) -> Vec<Span> {
    let bracketed = |value: String| {
        vec![
            Span::new(Style::Dim, "‹ "),
            Span::new(Style::Accent, value),
            Span::new(Style::Dim, " ›"),
        ]
    };
    match control {
        Control::Toggle(toggle) if toggle.on => vec![Span::new(Style::HeaderAccent, " ON ")],
        Control::Toggle(_) => vec![Span::new(Style::Dim, " OFF ")],
        Control::Choice(choice) => {
            let label = choice
                .options
                .get(choice.index)
                .map(|option| option.label.clone());
            bracketed(label.unwrap_or_default())
        }
        Control::Number(number) => bracketed(format!("{:.*}", number.decimals, number.value)),
        Control::Text(text) => match (text.value.is_empty(), caret) {
            (true, None) => vec![Span::new(Style::Dim, DEFAULT_TEXT)],
            (true, Some(_)) => vec![Span::new(Style::Accent, TEXT_CARET)],
            (false, None) => vec![Span::new(Style::Normal, text.value.clone())],
            (false, Some(at)) => {
                let before: String = text.value.chars().take(at).collect();
                let after: String = text.value.chars().skip(at).collect();
                vec![
                    Span::new(Style::Normal, before),
                    Span::new(Style::Accent, TEXT_CARET),
                    Span::new(Style::Normal, after),
                ]
            }
        },
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_model::paint::row_text;

    use super::*;

    fn page() -> SettingsEditor {
        SettingsEditor::new(SettingsPage::new(vec![
            SettingsField::toggle("ligatures", "Ligatures", false).in_section("Font"),
            SettingsField::text("shell", "Shell", String::new()).in_section("Terminal"),
        ]))
    }

    fn press(editor: &mut SettingsEditor, code: KeyCode) {
        editor.on_key(&Key::plain(code));
    }

    #[test]
    fn test_changing_a_row_asks_the_window_to_keep_it_in_the_page_s_scope() {
        let mut editor = page();
        press(&mut editor, KeyCode::Char('l'));
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
    fn test_a_text_row_takes_typing_only_in_insert_until_esc() {
        let mut editor = page();
        press(&mut editor, KeyCode::Char('j'));
        press(&mut editor, KeyCode::Char('x'));
        assert!(editor.take_requests().is_empty(), "Normal types nothing");
        press(&mut editor, KeyCode::Char('i'));
        assert_eq!(editor.status().mode, Some(EditMode::Insert));
        press(&mut editor, KeyCode::Char('z'));
        let asked = editor.take_requests();
        assert!(
            matches!(&asked[..], [EditorRequest::Setting(edit)] if edit.value == "z"),
            "{asked:?}"
        );
        press(&mut editor, KeyCode::Escape);
        press(&mut editor, KeyCode::Char('k'));
        assert_eq!(editor.page.selected, 0, "Normal moves again");
    }

    #[test]
    fn test_the_selected_row_is_scrolled_into_view() {
        let fields = (0..40)
            .map(|n| SettingsField::toggle(&format!("k{n}"), &format!("Setting {n}"), false))
            .collect();
        let mut editor = SettingsEditor::new(SettingsPage::new(fields));
        for _ in 0..30 {
            press(&mut editor, KeyCode::Char('j'));
        }
        let frame = editor.content(10, 60, false);
        let line = frame.cursor_line.expect("a cursor line");
        assert!(line < 10, "in view");
        assert!(row_text(&frame.rows[line]).contains("Setting 30"));
    }

    #[test]
    fn test_refreshing_keeps_the_row_and_drops_a_scope_no_longer_there() {
        let mut editor = SettingsEditor::new(
            SettingsPage::new(page().page.fields)
                .with_workspace("w".to_string(), Default::default()),
        );
        press(&mut editor, KeyCode::Char('j'));
        press(&mut editor, KeyCode::Tab);
        assert_eq!(editor.page.scope, Scope::Workspace);
        editor.refresh(SettingsPage::new(page().page.fields));
        assert_eq!((editor.page.selected, editor.page.scope), (1, Scope::User));
    }
}
