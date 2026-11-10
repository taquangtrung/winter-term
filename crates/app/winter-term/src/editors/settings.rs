//! The Settings page: every setting of Winter's and of its extensions, a
//! row each under its section, changed where it stands. In Normal, `j` and
//! `k` move between rows, `h` and `l` (or `SPC`) change the row's value, and
//! `i` edits a text row, in Insert until `Esc`. `TAB` chooses where edits go
//! in a window with a workspace, `settings.kdl` or the workspace, and
//! `S-<delete>` drops a workspace's own value of the row. Every control
//! answers a click as its keys do.
//!
//! The page is drawn as HTML (see [`PageView`]): a switch for what is on or
//! off, segments or arrows for a choice, steps for a number, and a field for
//! text, the selected row in a thin accent border.
//!
//! The page keeps nothing of its own: each change is asked of the window
//! ([`EditorRequest::Setting`]), which puts it in force, keeps it, and hands
//! the page back the values in force (see [`SettingsEditor::refresh`]).

use std::fmt::Write;

use winter_model::editor::{
    Appearance, EditorOutcome, EditorRequest, EditorStatus, Page, ScrollPosition, SettingEdit,
};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::input::{Key, KeyCode, Mods};
use winter_model::paint::{Frame, Point, Row};
use winter_model::picture::Tile;
use winter_model::settings_page::{
    Control, Scope, SettingsField, SettingsPage, TextControl, TextKind,
};
use winter_model::textedit::modal::EditMode;

use super::web_page::{escape, PageView};

// ========================================================================
// Constants
// ========================================================================

/// What the page and its tab are called.
const TITLE: &str = "Settings";

/// What marks a row the workspace sets over `settings.kdl`.
const OVERRIDDEN_MARK: &str = "●";

/// The scope tabs' names.
const USER_SCOPE: &str = "User";
const WORKSPACE_SCOPE: &str = "Workspace";

/// What an empty text row shows, which leaves the default in force.
const DEFAULT_TEXT: &str = "default";

/// The most options a choice shows side by side; one with more steps
/// through them between arrows.
const SEGMENTS_LIMIT: usize = 4;

/// The keys said under the title, by what the page is doing.
const HINT: &str = "j/k Move   h/l Change   SPC Toggle   i Edit Text";
const HINT_SCOPED: &str = "j/k Move   h/l Change   SPC Toggle   i Edit Text   TAB Scope";
const HINT_WORKSPACE: &str =
    "j/k Move   h/l Change   i Edit Text   TAB Scope   S-<delete> Use User's";
const HINT_INSERT: &str = "Typing into the setting   Esc Done";

/// What a click names, before the row it is on: `forward:3`, `choose:3:1`.
const CLICK_SCOPE: &str = "scope";
const CLICK_SELECT: &str = "select";
const CLICK_BACK: &str = "back";
const CLICK_FORWARD: &str = "forward";
const CLICK_CHOOSE: &str = "choose";
const CLICK_EDIT: &str = "edit";
const CLICK_ADD: &str = "add";
const CLICK_REMOVE: &str = "remove";

/// The page's own look, over the theme's stylesheet.
const STYLE: &str = "
.head { display: flex; align-items: baseline; gap: 20px; margin: 4px 0 2px; }
.title { font-size: 20px; font-weight: 600; }
.scope { color: var(--muted); padding: 1px 8px; border: 1px solid transparent; border-radius: 4px; }
.scope.chosen { color: var(--fg); border-color: var(--accent); }
.hint { color: var(--muted); font-size: 12px; margin: 2px 0 10px; }
h2 { font-size: 12px; font-weight: 600; letter-spacing: 0.06em; text-transform: uppercase;
  color: var(--accent); margin: 18px 0 4px; }
.row { display: flex; align-items: center; gap: 16px; padding: 7px 10px;
  border: 1px solid transparent; border-radius: 6px; }
.row.selected { border-color: var(--accent); background: var(--tint); }
.text { flex: 1; min-width: 0; }
.mark { color: var(--accent); margin-left: 6px; }
.note { color: var(--muted); font-size: 12px; margin-top: 2px; }
.control { flex: none; display: flex; align-items: center; gap: 4px; }
.switch { width: 34px; height: 18px; border-radius: 9px; background: var(--border); position: relative; }
.switch.on { background: var(--accent); }
.knob { position: absolute; top: 2px; left: 2px; width: 14px; height: 14px; border-radius: 7px;
  background: var(--bg); }
.switch.on .knob { left: 18px; }
.segment, .step { padding: 1px 9px; border: 1px solid var(--border); border-radius: 4px; color: var(--muted); }
.segment.chosen { color: var(--fg); border-color: var(--accent); }
.value { min-width: 64px; text-align: center; }
.field { min-width: 220px; padding: 2px 8px; border: 1px solid var(--border); border-radius: 4px;
  font-family: monospace; }
.field.editing { border-color: var(--accent); }
.empty { color: var(--muted); }
.control { flex-wrap: wrap; justify-content: flex-end; max-width: 60%; }
.name { padding: 1px 4px 1px 8px; border: 1px solid var(--border); border-radius: 4px; }
.remove { color: var(--muted); padding: 0 4px; }
.offer { padding: 1px 8px; border: 1px dashed var(--border); border-radius: 4px; color: var(--muted); }
.swatch { width: 16px; height: 16px; border: 1px solid var(--border); border-radius: 3px; }
.caret { display: inline-block; width: 0; height: 1.1em; border-left: 1px solid var(--accent);
  vertical-align: text-bottom; }
";

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

/// What a click on a row's control does.
#[derive(Clone, Debug, Eq, PartialEq)]
enum ClickAct {
    /// Add a name to a names row.
    Add(String),
    Back,
    /// Set a choice to its option, by its place.
    Choose(usize),
    Edit,
    Forward,
    /// Take a names row's name away, by its place.
    Remove(usize),
    Select,
}

/// The Settings page.
pub struct SettingsEditor {
    /// Whether a text row is being typed into: Insert, until `Esc`.
    inserting: bool,
    page: SettingsPage,
    /// What the page has asked of the window, not yet taken.
    requests: Vec<EditorRequest>,
    /// The page drawn as HTML.
    view: PageView,
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
            view: PageView::default(),
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

    /// Ask the window to keep `edit`, unless the selected row is kept only
    /// when its editing ends.
    fn ask_unless_deferred(&mut self, edit: Option<(String, String)>) {
        if self.page.deferred_edit().is_none() {
            self.ask(edit);
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

    /// Do what a click on the element naming `action` asks: switch the
    /// scope, or select its row and work its control as the keys would.
    fn clicked(&mut self, action: &str) {
        let mut parts = action.split(':');
        let verb = parts.next().unwrap_or_default();
        if verb == CLICK_SCOPE {
            self.inserting = false;
            self.page.toggle_scope();
            return;
        }
        let Some(row) = parts.next().and_then(|row| row.parse::<usize>().ok()) else {
            return;
        };
        let argument = parts.next();
        let option = argument.and_then(|option| option.parse::<usize>().ok());
        let act = match (verb, option, argument) {
            (CLICK_ADD, _, Some(name)) => ClickAct::Add(name.to_string()),
            (CLICK_BACK, _, _) => ClickAct::Back,
            (CLICK_CHOOSE, Some(option), _) => ClickAct::Choose(option),
            (CLICK_EDIT, _, _) => ClickAct::Edit,
            (CLICK_FORWARD, _, _) => ClickAct::Forward,
            (CLICK_REMOVE, Some(index), _) => ClickAct::Remove(index),
            (CLICK_SELECT, _, _) => ClickAct::Select,
            _ => return,
        };
        if row >= self.page.fields.len() {
            return;
        }
        if row != self.page.selected {
            self.inserting = false;
            self.page.selected = row;
        }
        match act {
            ClickAct::Add(name) => {
                let edit = self.page.add_name(&name);
                self.ask(edit);
            }
            ClickAct::Back => {
                self.run(Act::Back);
            }
            ClickAct::Choose(option) => {
                let edit = self.page.choose(option);
                self.ask(edit);
            }
            ClickAct::Edit => self.inserting = self.page.selected_is_text(),
            ClickAct::Forward => {
                self.run(Act::Forward);
            }
            ClickAct::Remove(index) => {
                let edit = self.page.remove_name(index);
                self.ask(edit);
            }
            ClickAct::Select => {}
        }
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

    /// The page as HTML: the title and its scope tabs, the keys, then each
    /// section's rows.
    fn html(&self) -> String {
        let mut out = format!(
            "<style>{STYLE}</style><div class=\"head\"><span class=\"title\">{TITLE}</span>"
        );
        if let Some(name) = self.page.workspace.as_deref() {
            let tab = |chosen: bool, label: &str| {
                let chosen = if chosen { " chosen" } else { "" };
                format!(
                    "<span class=\"scope{chosen}\" data-action=\"{CLICK_SCOPE}\">{label}</span>"
                )
            };
            out.push_str(&tab(self.page.scope == Scope::User, USER_SCOPE));
            let workspace = format!("{WORKSPACE_SCOPE}: {}", escape(name));
            out.push_str(&tab(self.page.scope == Scope::Workspace, &workspace));
        }
        let _ = write!(
            out,
            "</div><div class=\"hint\">{}</div>",
            escape(self.hint())
        );
        let mut section: Option<&str> = None;
        for (at, field) in self.page.fields.iter().enumerate() {
            if let Some(name) = field
                .section
                .as_deref()
                .filter(|name| section != Some(name))
            {
                section = Some(name);
                let _ = write!(out, "<h2>{}</h2>", escape(name));
            }
            out.push_str(&self.row_html(at, field));
        }
        out
    }

    /// One setting's row: its label, the mark of a workspace's own value,
    /// its note, and its control.
    fn row_html(&self, at: usize, field: &SettingsField) -> String {
        let selected = at == self.page.selected;
        let (class, current) = match selected {
            true => ("row selected", " data-current"),
            false => ("row", ""),
        };
        let mark = match self.page.overridden.contains(&field.key) {
            true => format!("<span class=\"mark\">{OVERRIDDEN_MARK}</span>"),
            false => String::new(),
        };
        let note = field
            .note
            .as_deref()
            .map(|note| format!("<div class=\"note\">{}</div>", escape(note)))
            .unwrap_or_default();
        let caret = (selected && self.inserting)
            .then(|| self.page.text_caret())
            .flatten();
        format!(
            "<div class=\"{class}\"{current} data-action=\"{CLICK_SELECT}:{at}\">\
             <div class=\"text\"><div>{}{mark}</div>{note}</div>\
             <div class=\"control\">{}</div></div>",
            escape(&field.label),
            control_html(at, &field.control, caret, selected),
        )
    }
}

impl Page for SettingsEditor {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        self.view.show(self.html(), (rows, cols));
        Frame::new(vec![Row::new()])
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        if self.inserting {
            // key-table: Insert's own way back to Normal, as every editor's is.
            if key.chord() == (Mods::NONE, KeyCode::Escape) {
                self.inserting = false;
                let kept = self.page.deferred_edit();
                self.ask(kept);
            } else {
                let edit = self.page.edit_text(key);
                self.ask_unless_deferred(edit);
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
            self.ask_unless_deferred(edit);
        }
        EditorOutcome::Consumed
    }

    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        // A press, not the drag after it.
        if !at.drag {
            if let Some(action) = self.view.action_at(at) {
                self.clicked(&action);
            }
        }
        EditorOutcome::Consumed
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        self.view.scroll_rows(lines);
        EditorOutcome::Consumed
    }

    fn scrolls_smoothly(&self) -> bool {
        true
    }

    fn on_scroll_pixels(&mut self, pixels: f32) -> EditorOutcome {
        self.view.scroll_by(pixels);
        EditorOutcome::Consumed
    }

    fn scroll_position(&self) -> Option<ScrollPosition> {
        self.view.scroll_position()
    }

    fn scroll_to(&mut self, offset: usize) -> EditorOutcome {
        self.view.scroll_to(offset as f32);
        EditorOutcome::Consumed
    }

    fn tiles(&self) -> Vec<Tile> {
        self.view.tiles()
    }

    fn set_appearance(&mut self, appearance: &Appearance) {
        self.view.set_appearance(appearance.theme, appearance.scale);
    }

    fn set_pane_size(&mut self, pane: (f32, f32)) {
        self.view.set_pane_size(pane);
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

/// The control of row `at`: a switch, a choice's segments or arrows, a
/// number's steps, or a text field with the caret at `caret` while it is
/// edited, each part naming what a click on it does. A names row shows its
/// names one by one, and when `selected` offers the ones it may add.
fn control_html(at: usize, control: &Control, caret: Option<usize>, selected: bool) -> String {
    let step = |verb: &str, label: &str| {
        format!("<span class=\"step\" data-action=\"{verb}:{at}\">{label}</span>")
    };
    match control {
        Control::Toggle(toggle) => {
            let on = if toggle.on { " on" } else { "" };
            format!("<span class=\"switch{on}\" data-action=\"{CLICK_FORWARD}:{at}\"><span class=\"knob\"></span></span>")
        }
        Control::Choice(choice) if choice.options.len() <= SEGMENTS_LIMIT => choice
            .options
            .iter()
            .enumerate()
            .map(|(option, shown)| {
                let chosen = if option == choice.index { " chosen" } else { "" };
                format!(
                    "<span class=\"segment{chosen}\" data-action=\"{CLICK_CHOOSE}:{at}:{option}\">{}</span>",
                    escape(&shown.label)
                )
            })
            .collect(),
        Control::Choice(choice) => {
            let label = choice
                .options
                .get(choice.index)
                .map(|option| option.label.as_str())
                .unwrap_or_default();
            format!(
                "{}<span class=\"value\">{}</span>{}",
                step(CLICK_BACK, "‹"),
                escape(label),
                step(CLICK_FORWARD, "›")
            )
        }
        Control::Number(number) => format!(
            "{}<span class=\"value\">{:.*}</span>{}",
            step(CLICK_BACK, "−"),
            number.decimals,
            number.value,
            step(CLICK_FORWARD, "+")
        ),
        Control::Text(text) if caret.is_none() && matches!(text.kind, TextKind::Names(_)) => {
            names_html(at, text, selected)
        }
        Control::Text(text) => {
            let swatch = match (&text.kind, text.value.is_empty()) {
                (TextKind::Color, false) => format!(
                    "<span class=\"swatch\" style=\"background: {}\"></span>",
                    escape(&text.value)
                ),
                _ => String::new(),
            };
            let (class, shown) = match (text.value.is_empty(), caret) {
                (true, None) => ("field", format!("<span class=\"empty\">{DEFAULT_TEXT}</span>")),
                (_, None) => ("field", escape(&text.value)),
                (_, Some(at)) => {
                    let before: String = text.value.chars().take(at).collect();
                    let after: String = text.value.chars().skip(at).collect();
                    (
                        "field editing",
                        format!(
                            "{}<span class=\"caret\"></span>{}",
                            escape(&before),
                            escape(&after)
                        ),
                    )
                }
            };
            format!(
                "{swatch}<span class=\"{class}\" data-action=\"{CLICK_EDIT}:{at}\">{shown}</span>"
            )
        }
    }
}

/// A names row's names, each with a mark that takes it away, then, when
/// the row is `selected`, the names it may take that it does not hold.
fn names_html(at: usize, text: &TextControl, selected: bool) -> String {
    let held: Vec<&str> = text.value.split_whitespace().collect();
    let mut out = String::new();
    for (index, name) in held.iter().enumerate() {
        let _ = write!(
            out,
            "<span class=\"name\">{}<span class=\"remove\" data-action=\"{CLICK_REMOVE}:{at}:{index}\">×</span></span>",
            escape(name)
        );
    }
    if held.is_empty() {
        let _ = write!(
            out,
            "<span class=\"empty\" data-action=\"{CLICK_EDIT}:{at}\">{DEFAULT_TEXT}</span>"
        );
    }
    let TextKind::Names(choices) = &text.kind else {
        return out;
    };
    if selected {
        for choice in choices
            .iter()
            .filter(|choice| !held.contains(&choice.as_str()))
        {
            let _ = write!(
                out,
                "<span class=\"offer\" data-action=\"{CLICK_ADD}:{at}:{}\">+ {}</span>",
                escape(choice),
                escape(choice)
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
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
    fn test_the_page_marks_the_selected_row_and_draws_each_control() {
        let mut editor = page();
        let html = editor.html();
        assert!(html.contains("class=\"row selected\" data-current data-action=\"select:0\""));
        assert!(html.contains("class=\"switch\" data-action=\"forward:0\""));
        assert!(html.contains(">default<"), "an empty text row says so");
        press(&mut editor, KeyCode::Char('j'));
        press(&mut editor, KeyCode::Char('i'));
        assert!(editor.html().contains("<span class=\"caret\"></span>"));
    }

    #[test]
    fn test_a_click_selects_its_row_and_works_its_control_as_the_keys_do() {
        let mut editor = page();
        editor.clicked("forward:0");
        assert!(
            matches!(&editor.take_requests()[..], [EditorRequest::Setting(edit)] if edit.value == "true")
        );
        editor.clicked("edit:1");
        assert_eq!((editor.page.selected, editor.inserting), (1, true));
        editor.clicked("select:0");
        assert!(!editor.inserting, "leaving a row leaves Insert");
        editor.clicked("forward:9");
        editor.clicked("nonsense");
        assert!(editor.take_requests().is_empty());
    }

    #[test]
    fn test_a_names_row_shows_its_names_and_offers_the_rest_when_selected() {
        let held = vec!["git".to_string()];
        let mut editor = SettingsEditor::new(SettingsPage::new(vec![
            SettingsField::names("seg", "Segments", &held, vec!["git".into(), "clock".into()]),
            SettingsField::toggle("k", "K", false),
        ]));
        let html = editor.html();
        assert!(html.contains("data-action=\"remove:0:0\""));
        assert!(html.contains("data-action=\"add:0:clock\""));
        assert!(!html.contains("add:0:git"), "a name held is not offered");
        editor.clicked("add:0:clock");
        assert!(
            matches!(&editor.take_requests()[..], [EditorRequest::Setting(edit)] if edit.value == "git clock")
        );
        editor.clicked("select:1");
        assert!(
            !editor.html().contains("add:0:"),
            "offered only on the selected row"
        );
    }

    #[test]
    fn test_a_deferred_row_is_kept_only_when_its_editing_ends() {
        let mut editor = SettingsEditor::new(SettingsPage::new(vec![SettingsField::text(
            "new",
            "Add",
            String::new(),
        )
        .deferred()]));
        press(&mut editor, KeyCode::Char('i'));
        press(&mut editor, KeyCode::Char('x'));
        assert!(editor.take_requests().is_empty(), "not at every key");
        press(&mut editor, KeyCode::Escape);
        assert!(
            matches!(&editor.take_requests()[..], [EditorRequest::Setting(edit)] if edit.value == "x")
        );
    }

    #[test]
    fn test_a_color_row_shows_a_swatch_of_its_color() {
        let editor = SettingsEditor::new(SettingsPage::new(vec![SettingsField::color(
            "colors.background",
            "background",
            "#123456".into(),
        )]));
        assert!(editor
            .html()
            .contains("class=\"swatch\" style=\"background: #123456\""));
    }

    #[test]
    fn test_a_label_s_markup_is_shown_as_text() {
        let editor = SettingsEditor::new(SettingsPage::new(vec![SettingsField::toggle(
            "k",
            "<b>bold</b> & co",
            false,
        )]));
        assert!(editor.html().contains("&lt;b&gt;bold&lt;/b&gt; &amp; co"));
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
