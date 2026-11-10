//! The Extensions page: every extension installed as a file, whether it
//! runs, and what it may reach, each grant a row: what it asked, what the
//! user added, and what it asked and was refused. `RET` on a refused grant
//! grants it, `d` takes a grant back, `D` removes the extension, and the
//! rows under each grant it a directory, asked for as a path. The first row
//! installs one from its component, through the window's installer.
//!
//! The page acts on Winter's own state, through `extensions::manage`, as the
//! command line does, and reads it again after every change; what changes
//! takes effect when Winter next starts.

use winter_model::editor::{
    EditorOutcome, EditorStatus, Notice, Page, PromptMode, PromptReply, PromptRequest,
};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::input::Key;
use winter_model::paint::{Frame, Row, Span, Style};

use crate::extensions::manage::{self, Installed, Places, Standing};
use crate::extensions::package::GrantKind;

// ========================================================================
// Constants
// ========================================================================

/// What the page and its tab are called.
const TITLE: &str = "Extensions";

/// The keys said under the title.
const HINT: &str = "j/k Move   RET Act on the Row   d Take Back   D Remove   \
                    Changes take effect when Winter restarts";

/// The palette command that installs an extension from its component.
const INSTALL_COMMAND: &str = "extension_install";

/// The rows above the extensions: the title, the keys, and a blank.
const HEADER_ROWS: usize = 3;

/// How far a grant's row is set in under its extension's.
const GRANT_INDENT: &str = "      ";
const EXTENSION_INDENT: &str = "  ";

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
            keys: &["d"],
            label: "Take the Grant Back",
        },
        Binding {
            action: Act::Remove,
            id: "remove_extension",
            keys: &["D"],
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
    /// The directory kind being asked for, and of which extension.
    asking: Option<GrantPlace>,
    notice: Option<Notice>,
    places: Places,
    lines: Vec<Line>,
    selected: usize,
    /// The first line shown, scrolled to keep the selected one in view.
    top: usize,
}

/// One line of the page, and what `RET` and the other keys do on it.
struct Line {
    act: Option<LineAct>,
    /// The extension the line is under.
    extension: Option<String>,
    row: Row,
}

/// What a line offers.
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
        let mut page = Self {
            asking: None,
            lines: Vec::new(),
            notice: None,
            places,
            selected: 0,
            top: 0,
        };
        page.reread();
        page
    }

    /// Read what is installed and granted again, keeping the place.
    pub(crate) fn reread(&mut self) {
        self.lines = match manage::installed(&self.places) {
            Ok(installed) => lines(&installed),
            Err(why) => {
                self.notice = Some(Notice::error(why));
                lines(&[])
            }
        };
        self.selected = self.selected.min(self.lines.len().saturating_sub(1));
    }

    /// Say `done` went through, or why it did not, and read the state again.
    fn after(&mut self, done: Result<String, String>) {
        self.notice = Some(match done {
            Ok(said) => Notice::info(format!("{said}: restart Winter for it to take effect")),
            Err(why) => Notice::error(why),
        });
        self.reread();
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
}

impl Page for ExtensionsPage {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn content(&mut self, rows: usize, _cols: usize, _wrap: bool) -> Frame {
        let shown = rows.saturating_sub(HEADER_ROWS).max(1);
        self.top = self.top.min(self.selected);
        if self.selected >= self.top + shown {
            self.top = self.selected + 1 - shown;
        }
        let mut frame_rows = vec![
            vec![
                Span::plain(EXTENSION_INDENT),
                Span::new(Style::HeaderAccent, TITLE),
            ],
            vec![Span::plain(EXTENSION_INDENT), Span::new(Style::Dim, HINT)],
            Vec::new(),
        ];
        frame_rows.extend(
            self.lines
                .iter()
                .skip(self.top)
                .take(shown)
                .map(|line| line.row.clone()),
        );
        Frame::new(frame_rows).with_cursor_line(HEADER_ROWS + self.selected - self.top)
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        match table::find(&KEYS, key) {
            Some(act) => self.run(act),
            None => EditorOutcome::Ignored,
        }
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
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

    fn on_resume(&mut self) -> EditorOutcome {
        self.reread();
        EditorOutcome::Consumed
    }

    fn keys(&self) -> Vec<KeyHelp> {
        table::help(&KEYS)
    }

    fn picks_rows(&self) -> bool {
        true
    }

    fn status(&self) -> EditorStatus {
        EditorStatus {
            mode: None,
            segments: Vec::new(),
        }
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The page's lines over `installed`: the installer first, then each
/// extension and its grants, and the rows to grant it a directory.
fn lines(installed: &[Installed]) -> Vec<Line> {
    let mut lines = vec![Line {
        act: Some(LineAct::Install),
        extension: None,
        row: vec![
            Span::plain(EXTENSION_INDENT),
            Span::new(Style::Accent, "+ Install From File…"),
        ],
    }];
    if installed.is_empty() {
        lines.push(Line {
            act: None,
            extension: None,
            row: vec![
                Span::plain(EXTENSION_INDENT),
                Span::new(Style::Dim, "No extension is installed as a file"),
            ],
        });
    }
    for one in installed {
        let id = Some(one.id.clone());
        let standing = match one.standing {
            Standing::Approved => (Style::Dim, "approved"),
            Standing::Changed => (
                Style::Diagnostic(winter_model::paint::DiagnosticLevel::Warning),
                "changed since approved: not run",
            ),
            Standing::NotApproved => (
                Style::Diagnostic(winter_model::paint::DiagnosticLevel::Warning),
                "not approved: not run",
            ),
        };
        lines.push(Line {
            act: None,
            extension: None,
            row: Vec::new(),
        });
        lines.push(Line {
            act: None,
            extension: id.clone(),
            row: vec![
                Span::plain(EXTENSION_INDENT),
                Span::new(Style::Header, one.id.clone()),
                Span::plain("  "),
                Span::new(standing.0, standing.1),
            ],
        });
        let Some(approval) = &one.approval else {
            continue;
        };
        let grant_line = |kind: GrantKind, value: &String, act: LineAct, note: &'static str| Line {
            act: Some(act),
            extension: id.clone(),
            row: vec![
                Span::plain(GRANT_INDENT),
                Span::new(Style::Normal, format!("{kind}  {value}")),
                Span::plain("  "),
                Span::new(Style::Dim, note),
            ],
        };
        let grant = |kind: GrantKind, value: &String| Grant {
            id: one.id.clone(),
            kind,
            value: value.clone(),
        };
        for kind in GrantKind::ALL {
            for value in approval.granted.of(kind) {
                let note = match approval.asked.of(kind).contains(value) {
                    true => "asked",
                    false => "added by you",
                };
                lines.push(grant_line(
                    kind,
                    value,
                    LineAct::Granted(grant(kind, value)),
                    note,
                ));
            }
        }
        let refused = approval.asked.beyond(&approval.granted);
        for kind in GrantKind::ALL {
            for value in refused.of(kind) {
                let act = LineAct::Refused(grant(kind, value));
                lines.push(grant_line(
                    kind,
                    value,
                    act,
                    "asked, not granted: RET grants it",
                ));
            }
        }
        for (kind, label) in [
            (GrantKind::Read, "+ Grant a directory to read…"),
            (GrantKind::Write, "+ Grant a directory to read and write…"),
        ] {
            lines.push(Line {
                act: Some(LineAct::AddDir(GrantPlace {
                    id: one.id.clone(),
                    kind,
                })),
                extension: id.clone(),
                row: vec![Span::plain(GRANT_INDENT), Span::new(Style::Accent, label)],
            });
        }
    }
    lines
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use winter_model::input::KeyCode;

    use super::*;
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

    fn select(page: &mut ExtensionsPage, wanted: impl Fn(&LineAct) -> bool) {
        page.selected = page
            .lines
            .iter()
            .position(|line| line.act.as_ref().is_some_and(&wanted))
            .expect("a row for it");
    }

    #[test]
    fn test_a_grant_taken_back_can_be_granted_again_from_its_row() {
        let (_scratch, mut page) = installed("again");
        select(&mut page, |act| matches!(act, LineAct::Granted(_)));
        page.on_key(&Key::plain(KeyCode::Char('d')));
        let notice = page.take_notice().expect("said");
        assert!(notice.text.contains("restart Winter"), "{}", notice.text);
        select(&mut page, |act| matches!(act, LineAct::Refused(_)));
        page.on_key(&Key::plain(KeyCode::Enter));
        let approvals = crate::extensions::approvals::Approvals::load(&page.places.approvals);
        let granted = approvals
            .expect("kept")
            .get("notes")
            .expect("approved")
            .granted
            .clone();
        assert_eq!(granted.read, ["~/Notes"], "granted again");
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
        let approvals = crate::extensions::approvals::Approvals::load(&page.places.approvals);
        let granted = approvals
            .expect("kept")
            .get("notes")
            .expect("approved")
            .granted
            .clone();
        assert_eq!(granted.write, ["~/Notes/Data"]);
    }
}
