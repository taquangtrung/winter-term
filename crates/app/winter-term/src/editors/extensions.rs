//! The Extensions page: every extension installed as a file, whether it
//! runs, and what it may reach, each in a section of its own with a row for
//! each grant: what it asked, what the user added, and what it asked and
//! was refused. `RET` on a refused grant grants it, `d` takes a grant back,
//! `D` removes the extension, and the rows under each grant it a directory,
//! asked for as a path. The first row installs one from its component,
//! through the window's installer. Each row's button does what `RET` (or
//! `d`, or `D`) does there, and a double click does what `RET` does.
//!
//! Misty draws the page as a view ([`PageView`]); its keys are Winter's, in
//! the `extensions` block of `keybindings.kdl`, moving the page's own
//! cursor, which the view marks and keeps in sight.
//!
//! The page acts on Winter's own state, through `extensions::manage`, as the
//! command line does, and reads it again after every change; what changes
//! takes effect when Winter next starts.

use std::time::Instant;

use misty::view::{Item, Press, badge, button, column, item, page, section};
use misty::{
    AccessAction, ButtonVariant, Element, EventCx, Frame as MistyFrame, HostedView, Input, Rect,
};
use misty::{Theme, ThemeToken};

use winter_model::editor::{
    EditorOutcome, EditorStatus, Notice, Page, PageView, PromptMode, PromptReply, PromptRequest,
};
use winter_model::input::Key;
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::paint::{Frame, Row};

use super::notice::ExtensionsNotice;
use crate::extensions::manage::{self, Installed, Places, Standing};
use crate::extensions::package::GrantKind;

// ========================================================================
// Constants
// ========================================================================

/// What the page and its tab are called.
const TITLE: &str = "Extensions";

/// The palette command that installs an extension from its component.
const INSTALL_COMMAND: &str = "extension_install";

/// The section the installer's row is in, and what that row says.
const INSTALL_SECTION: &str = "Install";
const INSTALL_LABEL: &str = "Install From File…";
const INSTALL_NOTE: &str = "A WebAssembly component, approved before it runs";

/// What the page says with nothing installed.
const NOTHING_INSTALLED: &str = "No extension is installed as a file";

/// The section listing the extensions compiled into Winter, and what each
/// says of itself.
const BUILT_IN_SECTION: &str = "Built in";
const BUILT_IN_NOTE: &str = "Compiled into Winter; always on";

/// What each extension's first row says of it, by its standing.
const STANDING_APPROVED: &str = "Approved";
const STANDING_CHANGED: &str = "Changed since approved: not run";
const STANDING_NOT_APPROVED: &str = "Not approved: not run";

/// What a grant's row says of where it came from.
const NOTE_ASKED: &str = "asked";
const NOTE_ADDED: &str = "added by you";
const NOTE_REFUSED: &str = "asked, not granted: RET grants it";

/// The badges on a grant's row, and on an extension's first row by
/// whether it runs.
const BADGE_GRANTED: &str = "granted";
const BADGE_REFUSED: &str = "refused";
const BADGE_RUNS: &str = "runs";
const BADGE_STOPPED: &str = "stopped";

/// The rows that grant an extension a directory.
const ADD_READ: &str = "Grant a directory to read…";
const ADD_WRITE: &str = "Grant a directory to read and write…";

/// The rows' buttons.
const BUTTON_ADD: &str = "Add…";
const BUTTON_GRANT: &str = "Grant";
const BUTTON_INSTALL: &str = "Install…";
const BUTTON_REMOVE: &str = "Remove";
const BUTTON_TAKE_BACK: &str = "Take Back";

/// The clicks in a row that do what `RET` does on it.
const DOUBLE_CLICK: u8 = 2;

/// The prompts' tags.
const ASK_DIR: &str = "extension_grant_dir";
const ASK_REMOVE: &str = "extension_remove";

/// The page's keys, block `extensions`.
const KEYS: KeyTable<Act> = KeyTable {
    block: "extensions",
    bindings: &[
        Binding {
            action: Act::Down,
            id: "next_row",
            keys: &["j", "<down>"],
            label: "Next Row",
        },
        Binding {
            action: Act::Up,
            id: "previous_row",
            keys: &["k", "<up>"],
            label: "Previous Row",
        },
        Binding {
            action: Act::Run,
            id: "act_on_row",
            keys: &["RET"],
            label: "Install, Grant, or Add a Directory, as the Row Says",
        },
        Binding {
            action: Act::Revoke,
            id: "take_back",
            keys: &["x"],
            label: "Take the Grant Back",
        },
        Binding {
            action: Act::Remove,
            id: "remove_extension",
            keys: &["X"],
            label: "Remove the Extension",
        },
    ],
    configurable: true,
    title: "Extensions",
};

/// The page's key tables, for the Keys page and `keybindings.kdl`.
pub(crate) const KEY_TABLES: &[&dyn Listed] = &[&KEYS];

// ========================================================================
// Data Structures
// ========================================================================

/// What an extensions key does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    Down,
    Remove,
    Revoke,
    Run,
    Up,
}

/// The Extensions page.
pub(crate) struct ExtensionsPage {
    /// The page's view and what it shows.
    view: PageView<Shown>,
}

/// What the page shows and where its cursor is: the state its view is
/// built from and its keys and buttons change.
struct Shown {
    /// The directory kind being asked for, and of which extension.
    asking: Option<GrantPlace>,
    /// The ids of the extensions compiled into Winter, listed apart.
    built_in: Vec<&'static str>,
    /// The rows, top to bottom.
    lines: Vec<Line>,
    /// What to tell the user, until the window takes it.
    notice: Option<ExtensionsNotice>,
    /// What a button asked of the window, until the window takes it.
    outcome: Option<EditorOutcome>,
    /// Where the extensions and their approvals are kept.
    places: Places,
    /// The row the cursor is on.
    selected: usize,
}

/// One row of the page, and what `RET` and the other keys do on it.
struct Line {
    act: Option<LineAct>,
    /// The extension the row is under; `None` for the installer's.
    extension: Option<String>,
    /// What the row says.
    label: String,
    /// What it says under that, if anything.
    note: Option<&'static str>,
    /// The badge after the label, and its color, if any.
    tag: Option<(&'static str, ThemeToken)>,
}

/// What a row offers.
#[derive(Clone, Debug, Eq, PartialEq)]
enum LineAct {
    /// Ask for a directory to grant.
    AddDir(GrantPlace),
    /// A grant the extension holds, to take back.
    Granted(Grant),
    /// Open the installer.
    Install,
    /// A grant the extension asked and was refused, to grant.
    Refused(Grant),
}

/// A kind of grant to give one extension.
#[derive(Clone, Debug, Eq, PartialEq)]
struct GrantPlace {
    id: String,
    kind: GrantKind,
}

/// One grant of one extension.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Grant {
    id: String,
    kind: GrantKind,
    value: String,
}

// ========================================================================
// ExtensionsPage
// ========================================================================

impl ExtensionsPage {
    /// The page over the extensions installed at `places`.
    pub(crate) fn new(places: Places) -> Self {
        let mut shown = Shown {
            asking: None,
            built_in: crate::extensions::compiled_ids(),
            lines: Vec::new(),
            notice: None,
            outcome: None,
            places,
            selected: 0,
        };
        shown.reread();
        Self {
            view: PageView::new(shown),
        }
    }

    /// Read what is installed and granted again, keeping the place.
    pub(crate) fn reread(&mut self) {
        self.view.state_mut().reread();
    }
}

impl Page for ExtensionsPage {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    /// Misty draws the page; the grid under it is empty.
    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![Row::new()])
    }

    fn is_view(&self) -> bool {
        true
    }

    fn draw_view(&mut self, frame: &mut MistyFrame, area: Rect, theme: &Theme) {
        self.view.draw_view(frame, area, theme);
    }

    /// The view's buttons and clicks, and what they asked of the window;
    /// its keys go on to [`Page::on_key`].
    fn view_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Option<EditorOutcome> {
        let used = self.view.view_input(input, cx);
        let asked = self.view.state_mut().outcome.take();
        asked.or(used.then_some(EditorOutcome::Consumed))
    }

    fn view_access_action(&mut self, action: AccessAction) -> Option<EditorOutcome> {
        let used = self.view.access_action(action);
        let asked = self.view.state_mut().outcome.take();
        asked.or(used.then_some(EditorOutcome::Consumed))
    }

    fn view_about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.view.view_about_to_wait(cx)
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        match table::find(&KEYS, key) {
            Some(act) => self.view.state_mut().run(act),
            None => EditorOutcome::Ignored,
        }
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        self.view.state_mut().answer(reply)
    }

    fn on_resume(&mut self) -> EditorOutcome {
        self.reread();
        EditorOutcome::Consumed
    }

    fn keys(&self) -> Vec<KeyHelp> {
        table::help(&KEYS)
    }

    fn status(&self) -> EditorStatus {
        EditorStatus {
            mode: None,
            segments: Vec::new(),
        }
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.view
            .state_mut()
            .notice
            .take()
            .map(ExtensionsNotice::into_notice)
    }
}

// ========================================================================
// Shown
// ========================================================================

impl Shown {
    /// Read what is installed and granted again, keeping the place.
    fn reread(&mut self) {
        self.lines = match manage::installed(&self.places) {
            Ok(installed) => lines(&installed),
            Err(why) => {
                self.notice = Some(ExtensionsNotice::Failed(why));
                lines(&[])
            }
        };
        self.selected = self.selected.min(self.lines.len().saturating_sub(1));
    }

    /// Say `done` went through, or why it did not, and read the state again.
    fn after(&mut self, done: Result<String, String>) {
        self.notice = Some(match done {
            Ok(said) => ExtensionsNotice::Changed(said),
            Err(why) => ExtensionsNotice::Failed(why),
        });
        self.reread();
    }

    /// Do `act` on the row at `at`, as its button or a double click asks,
    /// keeping what it asks of the window for the window to take.
    fn run_at(&mut self, at: usize, act: Act) {
        self.selected = at;
        self.outcome = Some(self.run(act));
    }

    fn run(&mut self, act: Act) -> EditorOutcome {
        let line = self.lines.get(self.selected);
        match act {
            Act::Down => {
                self.selected = (self.selected + 1).min(self.lines.len().saturating_sub(1))
            }
            Act::Up => self.selected = self.selected.saturating_sub(1),
            Act::Remove => {
                let Some(id) = line.and_then(|line| line.extension.clone()) else {
                    return EditorOutcome::Consumed;
                };
                return EditorOutcome::Prompt(PromptRequest {
                    initial: String::new(),
                    label: format!("Remove {id} and forget what it was granted? (y/n)"),
                    mode: PromptMode::Confirm,
                    tag: ASK_REMOVE,
                    title: None,
                });
            }
            Act::Revoke => {
                if let Some(LineAct::Granted(grant)) = line.and_then(|line| line.act.clone()) {
                    let done = manage::revoke(&self.places, &grant.id, grant.kind, &grant.value);
                    self.after(done.map(|_| {
                        format!(
                            "{} no longer {}s {}",
                            grant.id,
                            grant.kind.name(),
                            grant.value
                        )
                    }));
                }
            }
            Act::Run => match line.and_then(|line| line.act.clone()) {
                Some(LineAct::AddDir(place)) => {
                    let label = format!("A directory for {} to {}", place.id, place.kind.name());
                    self.asking = Some(place);
                    return EditorOutcome::Prompt(PromptRequest {
                        initial: "~/".to_string(),
                        label,
                        mode: PromptMode::Text,
                        tag: ASK_DIR,
                        title: None,
                    });
                }
                Some(LineAct::Install) => {
                    return EditorOutcome::RunActionHere(INSTALL_COMMAND.to_string());
                }
                Some(LineAct::Refused(grant)) => {
                    let done = manage::grant(&self.places, &grant.id, grant.kind, &grant.value);
                    self.after(done.map(|_| {
                        format!("{} may {} {}", grant.id, grant.kind.name(), grant.value)
                    }));
                }
                Some(LineAct::Granted(_)) | None => {}
            },
        }
        EditorOutcome::Consumed
    }

    /// Take the answer a prompt the page opened was given.
    fn answer(&mut self, reply: PromptReply) -> EditorOutcome {
        let Some(answer) = reply.answer else {
            self.asking = None;
            return EditorOutcome::Consumed;
        };
        match reply.tag {
            ASK_DIR => {
                if let Some(place) = self.asking.take() {
                    let dir = answer.trim().trim_end_matches('/').to_string();
                    let done = manage::grant(&self.places, &place.id, place.kind, &dir);
                    self.after(
                        done.map(|_| format!("{} may {} {dir}", place.id, place.kind.name())),
                    );
                }
            }
            ASK_REMOVE => {
                let id = self
                    .lines
                    .get(self.selected)
                    .and_then(|line| line.extension.clone());
                if let Some(id) = id {
                    let done = manage::remove(&self.places, &id);
                    self.after(done.map(|()| format!("removed {id}")));
                }
            }
            _ => {}
        }
        EditorOutcome::Consumed
    }

    /// What the tooltip of the row at `at` says it means, or `None` for a
    /// row its label says all of.
    fn details(&self, at: usize) -> Option<String> {
        Some(match self.lines[at].act.as_ref()? {
            LineAct::Install => INSTALL_NOTE.to_string(),
            LineAct::AddDir(place) => {
                format!("Grants {} a directory to {}", place.id, place.kind.name())
            }
            LineAct::Granted(grant) => {
                format!("{} may {} {}", grant.id, grant.kind.name(), grant.value)
            }
            LineAct::Refused(grant) => format!(
                "{} asked to {} {}, and it was not granted",
                grant.id,
                grant.kind.name(),
                grant.value
            ),
        })
    }

    /// The row at `at` as the view shows it: its label, note, and badge,
    /// marked when the cursor is on it, a click putting the cursor there
    /// and a double click doing what `RET` does, with the button for what
    /// it offers.
    fn row_view(&self, at: usize) -> Item<'_, Self> {
        let line = &self.lines[at];
        let mut row = item(line.label.as_str())
            .selected(at == self.selected)
            .on_pointer_press(move |shown: &mut Self, press: Press| {
                shown.selected = at;
                if press.clicks >= DOUBLE_CLICK {
                    shown.run_at(at, Act::Run);
                }
            });
        // The selected row says its details too, for a keyboard user, who
        // sees no tooltip.
        let details = self
            .details(at)
            .filter(|details| at == self.selected && line.note != Some(details.as_str()));
        match (line.note, details) {
            (Some(note), Some(details)) => row = row.description(format!("{note} · {details}")),
            (Some(note), None) => row = row.description(note),
            (None, Some(details)) => row = row.description(details),
            (None, None) => {}
        }
        if let Some((tag, tone)) = line.tag {
            row = row.badge(badge(tag, tone));
        }
        // The selected row says its details already.
        let says_more = |details: &String| line.note != Some(details.as_str());
        if let Some(details) = self
            .details(at)
            .filter(|details| at != self.selected && says_more(details))
        {
            row = row.tooltip(details);
        }
        let action = match (&line.act, &line.extension) {
            (Some(LineAct::Install), _) => Some((BUTTON_INSTALL, Act::Run, ButtonVariant::Primary)),
            (Some(LineAct::AddDir(_)), _) => Some((BUTTON_ADD, Act::Run, ButtonVariant::Default)),
            (Some(LineAct::Granted(_)), _) => {
                Some((BUTTON_TAKE_BACK, Act::Revoke, ButtonVariant::Default))
            }
            (Some(LineAct::Refused(_)), _) => {
                Some((BUTTON_GRANT, Act::Run, ButtonVariant::Primary))
            }
            (None, Some(_)) => Some((BUTTON_REMOVE, Act::Remove, ButtonVariant::Danger)),
            (None, None) => None,
        };
        match action {
            Some((label, act, variant)) => row.trailing(
                button(label)
                    .variant(variant)
                    .on_click(move |shown: &mut Self| shown.run_at(at, act)),
            ),
            None => row,
        }
    }
}

impl HostedView for Shown {
    /// A section for the installer, then one for each extension, its rows
    /// in the order the cursor walks them.
    fn view(&self) -> Element<'_, Self> {
        let mut sections: Vec<Element<'_, Self>> = Vec::new();
        let mut at = 0;
        while at < self.lines.len() {
            let extension = self.lines[at].extension.as_deref();
            let title = extension.unwrap_or(INSTALL_SECTION);
            let mut group = section(title);
            // With nothing installed as a file, the installer's section says
            // so, rather than a row that looks chosen.
            let installed = self.lines.iter().any(|line| line.extension.is_some());
            if extension.is_none() && !installed {
                group = group.description(NOTHING_INSTALLED);
            }
            while at < self.lines.len() && self.lines[at].extension.as_deref() == extension {
                group = group.item(self.row_view(at));
                at += 1;
            }
            sections.push(group.into());
        }
        if !self.built_in.is_empty() {
            let rows = self.built_in.iter().map(|id| {
                item(*id)
                    .description(BUILT_IN_NOTE)
                    .badge(badge(BUILT_IN_SECTION, ThemeToken::Muted))
                    .into()
            });
            sections.push(section(BUILT_IN_SECTION).items(rows).into());
        }
        page(TITLE).scrolling(column(sections)).into()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The page's rows over `installed`: the installer first, then each
/// extension's standing and grants, and the rows to grant it a directory.
fn lines(installed: &[Installed]) -> Vec<Line> {
    let mut lines = vec![Line {
        act: Some(LineAct::Install),
        extension: None,
        label: INSTALL_LABEL.to_string(),
        note: Some(INSTALL_NOTE),
        tag: None,
    }];
    for one in installed {
        let id = Some(one.id.clone());
        let (standing, tone) = match one.standing {
            Standing::Approved => (STANDING_APPROVED, ThemeToken::Muted),
            Standing::Changed => (STANDING_CHANGED, ThemeToken::Warning),
            Standing::NotApproved => (STANDING_NOT_APPROVED, ThemeToken::Warning),
        };
        lines.push(Line {
            act: None,
            extension: id.clone(),
            label: standing.to_string(),
            note: None,
            tag: Some((badge_of(one.standing), tone)),
        });
        let Some(approval) = &one.approval else {
            continue;
        };
        let grant = |kind: GrantKind, value: &String| Grant {
            id: one.id.clone(),
            kind,
            value: value.clone(),
        };
        let grant_line = |kind: GrantKind, value: &String, act: LineAct, note, tag| Line {
            act: Some(act),
            extension: id.clone(),
            label: format!("{}  {value}", kind.name()),
            note: Some(note),
            tag: Some(tag),
        };
        for kind in GrantKind::ALL {
            for value in approval.granted.of(kind) {
                let note = match approval.asked.of(kind).contains(value) {
                    true => NOTE_ASKED,
                    false => NOTE_ADDED,
                };
                let act = LineAct::Granted(grant(kind, value));
                let tag = (BADGE_GRANTED, ThemeToken::Accent);
                lines.push(grant_line(kind, value, act, note, tag));
            }
        }
        let refused = approval.asked.beyond(&approval.granted);
        for kind in GrantKind::ALL {
            for value in refused.of(kind) {
                let act = LineAct::Refused(grant(kind, value));
                let tag = (BADGE_REFUSED, ThemeToken::Warning);
                lines.push(grant_line(kind, value, act, NOTE_REFUSED, tag));
            }
        }
        for (kind, label) in [(GrantKind::Read, ADD_READ), (GrantKind::Write, ADD_WRITE)] {
            lines.push(Line {
                act: Some(LineAct::AddDir(GrantPlace {
                    id: one.id.clone(),
                    kind,
                })),
                extension: id.clone(),
                label: label.to_string(),
                note: None,
                tag: None,
            });
        }
    }
    lines
}

/// The badge on an extension's first row, by its standing.
fn badge_of(standing: Standing) -> &'static str {
    match standing {
        Standing::Approved => BADGE_RUNS,
        Standing::Changed | Standing::NotApproved => BADGE_STOPPED,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use misty::Point;
    use misty::testing::TestWindow;
    use winter_model::input::KeyCode;

    use super::*;
    use crate::editors::view_tests::{click, drawn, middle_of, text_of};
    use crate::extensions::manage::module_with;

    /// Places of their own for one test, removed when it is done.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An extension `notes` installed at fresh places, asking to read
    /// `~/Notes`, and the page over it.
    fn installed(name: &str) -> (Scratch, ExtensionsPage) {
        let root =
            std::env::temp_dir().join(format!("winter-ext-page-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch");
        let places = Places {
            approvals: root.join("approvals.kdl"),
            extensions: root.join("extensions"),
        };
        let file = root.join("notes.wasm");
        let package = "package id=\"notes\" { permissions { read \"~/Notes\" } }";
        std::fs::write(&file, module_with(package)).expect("built");
        let review = manage::review(&file, &places).expect("reads");
        manage::install(review, &places).expect("installs");
        (Scratch(root), ExtensionsPage::new(places))
    }

    fn shown(page: &mut ExtensionsPage) -> &mut Shown {
        page.view.state_mut()
    }

    fn select(page: &mut ExtensionsPage, wanted: impl Fn(&LineAct) -> bool) {
        let shown = shown(page);
        shown.selected = shown
            .lines
            .iter()
            .position(|line| line.act.as_ref().is_some_and(&wanted))
            .expect("a row for it");
    }

    fn granted(page: &mut ExtensionsPage) -> crate::extensions::package::Grants {
        let approvals =
            crate::extensions::approvals::Approvals::load(&shown(page).places.approvals);
        approvals
            .expect("kept")
            .get("notes")
            .expect("approved")
            .granted
            .clone()
    }

    #[test]
    fn test_a_grant_taken_back_can_be_granted_again_from_its_row() {
        let (_scratch, mut page) = installed("again");
        select(&mut page, |act| matches!(act, LineAct::Granted(_)));
        page.on_key(&Key::plain(KeyCode::Char('x')));
        let notice = page.view.state_mut().notice.take();
        assert!(
            matches!(notice, Some(ExtensionsNotice::Changed(_))),
            "{notice:?}"
        );
        select(&mut page, |act| matches!(act, LineAct::Refused(_)));
        page.on_key(&Key::plain(KeyCode::Enter));
        assert_eq!(granted(&mut page).read, ["~/Notes"], "granted again");
    }

    #[test]
    fn test_a_directory_asked_for_is_granted_as_typed() {
        let (_scratch, mut page) = installed("dir");
        select(
            &mut page,
            |act| matches!(act, LineAct::AddDir(place) if place.kind == GrantKind::Write),
        );
        let asked = page.on_key(&Key::plain(KeyCode::Enter));
        assert!(
            matches!(asked, EditorOutcome::Prompt(_)),
            "asked for the path"
        );
        page.on_prompt(PromptReply {
            answer: Some("~/Notes/Data/".to_string()),
            tag: ASK_DIR,
        });
        assert_eq!(granted(&mut page).write, ["~/Notes/Data"]);
    }

    #[test]
    fn test_each_extension_is_a_section_of_its_rows_under_the_installer() {
        let (_scratch, mut page) = installed("sections");
        let window = TestWindow::new(1000, 800);
        let drawn = text_of(&drawn(&mut page, &window));
        // A closing ellipsis is drawn as a run of its own.
        let words = [
            INSTALL_SECTION,
            INSTALL_LABEL.trim_end_matches('…'),
            "notes",
            STANDING_APPROVED,
            "~/Notes",
            ADD_READ.trim_end_matches('…'),
        ];
        for words in words {
            assert!(drawn.contains(words), "{words:?} in {drawn:?}");
        }
    }

    #[test]
    fn test_a_click_on_a_row_puts_the_cursor_there_and_keys_stay_the_windows() {
        let (_scratch, mut page) = installed("click");
        let mut window = TestWindow::new(1000, 800);
        let frame = drawn(&mut page, &window);
        // The grant's row, left of its button.
        let point = Point::new(100.0, middle_of(&frame, BUTTON_TAKE_BACK).y);
        let (pressed, _) = click(&mut page, &mut window, point);
        assert!(pressed.is_some());
        let at = shown(&mut page).selected;
        assert!(matches!(
            shown(&mut page).lines[at].act,
            Some(LineAct::Granted(_))
        ));
        let key = Input::Key(misty::Key::new(
            misty::KeyCode::Char('j'),
            misty::Mods::NONE,
        ));
        assert_eq!(
            page.view_input(&key, &mut window.cx()),
            None,
            "j is Winter's"
        );
    }

    #[test]
    fn test_a_grant_s_tooltip_says_what_it_allows_and_a_plain_row_has_none() {
        let (_scratch, page) = installed("details");
        let shown = page.view.state();
        let at = |wanted: fn(Option<&LineAct>) -> bool| {
            shown
                .lines
                .iter()
                .position(|line| wanted(line.act.as_ref()))
                .expect("such a row")
        };
        let granted = at(|act| matches!(act, Some(LineAct::Granted(_))));
        assert_eq!(
            shown.details(granted).as_deref(),
            Some("notes may read ~/Notes")
        );
        assert_eq!(shown.details(at(|act| act.is_none())), None);
    }

    #[test]
    fn test_a_rows_button_asks_the_window_what_its_key_would() {
        let (_scratch, mut page) = installed("button");
        let mut window = TestWindow::new(1000, 800);
        let point = middle_of(&drawn(&mut page, &window), BUTTON_INSTALL);
        let (pressed, released) = click(&mut page, &mut window, point);
        assert_eq!(pressed, Some(EditorOutcome::Consumed));
        assert_eq!(
            released,
            Some(EditorOutcome::RunActionHere(INSTALL_COMMAND.to_string()))
        );
    }

    #[test]
    fn test_nothing_installed_is_said_not_listed_and_built_in_ones_are() {
        let root = std::env::temp_dir().join(format!("winter-ext-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch");
        let places = Places {
            approvals: root.join("approvals.kdl"),
            extensions: root.join("extensions"),
        };
        let mut page = ExtensionsPage::new(places);
        assert_eq!(
            page.view.state().lines.len(),
            1,
            "the installer alone to choose"
        );
        let shown = text_of(&drawn(&mut page, &TestWindow::new(1000, 800)));
        let _ = std::fs::remove_dir_all(&root);
        assert!(shown.contains(NOTHING_INSTALLED), "{shown:?}");
        assert!(
            shown.contains(BUILT_IN_SECTION) && shown.contains("probe"),
            "{shown:?}"
        );
        assert_eq!(
            shown.matches(INSTALL_NOTE).count(),
            1,
            "said once: {shown:?}"
        );
    }

    #[test]
    fn test_every_control_says_what_it_is_and_lights_under_the_pointer() {
        let (_scratch, mut page) = installed("named");
        let frame = drawn(&mut page, &TestWindow::new(1000, 800));
        let unnamed = misty::testing::unnamed_controls(&frame);
        assert!(unnamed.is_empty(), "{unnamed:?}");
        let mut window = TestWindow::new(1000, 800);
        let unlit = crate::editors::view_tests::unlit(&mut page, &mut window);
        assert!(unlit.is_empty(), "unlit under the pointer: {unlit:?}");
    }
}
