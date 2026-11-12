//! What a text tab asks the language server serving its file, and what it
//! does with the answers: going to where a name is defined, declared,
//! typed, or implemented, or picking among several such places; listing
//! its references; a card at the cursor of what the name is, or of the
//! call's signature; renaming it everywhere; formatting the file, before a
//! save where the settings ask it; and the fixes on offer at the cursor.
//! The places jumped from are kept, window wide, to go back and forth over.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use misty::text::{StyledText, TextColor};
use misty::theme::{Theme, ThemeToken};
use misty::{Frame, InfoCard, Rect, Widget};
use serde_json::{json, Value};
use viento::Utf16Position;
use winter_lsp::protocol::{self, CodeAction, FileEdits, Location, TextEdit};
use winter_lsp::{Port, ServerId};
use winter_model::editor::{
    EditorOutcome, EditorRequest, Notice, OpenTarget, PickPlacement, PickRequest, PromptMode,
    PromptReply, PromptRequest, LSP_CODE_ACTION, LSP_DECLARATION, LSP_DEFINITION, LSP_FORMAT,
    LSP_HOVER, LSP_IMPLEMENTATION, LSP_PEEK_DEFINITION, LSP_PEEK_REFERENCES, LSP_REFERENCES,
    LSP_RENAME, LSP_SIGNATURE, LSP_TYPE_DEFINITION, NAVIGATE_BACK, NAVIGATE_FORWARD,
};

use super::{file_edit, session, TextEditor};

// ========================================================================
// Constants
// ========================================================================

/// How long a save waits on the server formatting the file before it
/// writes the file as it is.
const FORMAT_SAVE_WAIT: Duration = Duration::from_secs(3);

/// The most places kept to go back to.
const MAX_JUMPS: usize = 100;

/// What the answers to the tab's lists and questions are told apart by.
const ASK_FIX: &str = "lsp-fix";
const ASK_PLACE: &str = "lsp-place";
const ASK_RENAME: &str = "lsp-rename";

/// What is said where no server can be asked.
const NOT_SERVED: &str = "no running language server serves this file";

/// What is said of edits worked out from text that has changed since.
const STALE_EDITS: &str = "the file changed while the server worked: ask again";

thread_local! {
    /// The places jumped from, to go back to, and those gone back from, to
    /// go forward to again: one list for every tab in the window.
    static JUMPS: RefCell<Jumps> = RefCell::new(Jumps::default());
}

// ========================================================================
// Data Structures
// ========================================================================

/// What a tab is waiting on its servers for, and what it shows of their
/// answers.
#[derive(Debug, Default)]
pub(super) struct Lsp {
    /// What each request in flight was for, by server and request id.
    asked: HashMap<(ServerId, u64), Asked>,
    /// The card open at the cursor.
    card: Option<Card>,
    /// The fixes on offer, by their titles, with the server offering them.
    fixes: Vec<(String, ServerId, CodeAction)>,
    /// The places last listed, by their labels.
    places: Vec<(String, OpenTarget)>,
    /// Edits to files this tab does not hold, for the window to make.
    pub(super) requests: Vec<EditorRequest>,
    /// When a save waiting on the server's formatting writes the file as
    /// it is.
    saving: Option<Instant>,
}

/// What a request in flight was for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Asked {
    /// The fixes at the cursor, of the text at a version.
    Actions(u64),
    /// A command the server runs, which sends its edits back.
    Command,
    /// The formatting of the text at a version.
    Format(u64),
    /// The formatting a save waits on, of the text at a version.
    FormatAndSave(u64),
    /// Where the name is defined, declared, typed, or implemented.
    Goto(&'static str),
    /// What the name at the cursor is.
    Hover,
    /// Where the name is used.
    References,
    /// The edits renaming the name, worked out from the text at a version.
    Rename(u64),
    /// The signature of the call at the cursor.
    Signature,
}

/// A card at the cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Card {
    /// A line under the text, set apart: where the problems came from.
    footer: Option<String>,
    /// What it says.
    text: String,
}

/// The places to go back and forward to.
#[derive(Debug, Default)]
struct Jumps {
    back: Vec<OpenTarget>,
    forward: Vec<OpenTarget>,
}

// ========================================================================
// TextEditor: asking the server
// ========================================================================

impl TextEditor {
    /// Runs the language server command `name`; `None` for another name.
    pub(super) fn run_lsp_command(&mut self, name: &str) -> Option<EditorOutcome> {
        let goto = |this: &mut Self, method, capability, what| {
            let params = this.here_params();
            this.ask(method, capability, params, Asked::Goto(what))
        };
        Some(match name {
            LSP_DEFINITION | LSP_PEEK_DEFINITION => goto(
                self,
                "textDocument/definition",
                "definitionProvider",
                "definition",
            ),
            LSP_DECLARATION => goto(
                self,
                "textDocument/declaration",
                "declarationProvider",
                "declaration",
            ),
            LSP_TYPE_DEFINITION => goto(
                self,
                "textDocument/typeDefinition",
                "typeDefinitionProvider",
                "type definition",
            ),
            LSP_IMPLEMENTATION => goto(
                self,
                "textDocument/implementation",
                "implementationProvider",
                "implementation",
            ),
            LSP_REFERENCES | LSP_PEEK_REFERENCES => {
                let mut params = self.here_params();
                params["context"] = json!({"includeDeclaration": name == LSP_REFERENCES});
                self.ask(
                    "textDocument/references",
                    "referencesProvider",
                    params,
                    Asked::References,
                )
            }
            LSP_HOVER => {
                let params = self.here_params();
                self.ask("textDocument/hover", "hoverProvider", params, Asked::Hover)
            }
            LSP_SIGNATURE => {
                let params = self.here_params();
                self.ask(
                    "textDocument/signatureHelp",
                    "signatureHelpProvider",
                    params,
                    Asked::Signature,
                )
            }
            LSP_RENAME => EditorOutcome::Prompt(PromptRequest {
                initial: self.word_here(),
                label: "Rename to: ".to_owned(),
                mode: PromptMode::Text,
                tag: ASK_RENAME,
                title: None,
            }),
            LSP_CODE_ACTION => self.ask_fixes(),
            LSP_FORMAT => {
                let params = self.format_params();
                let version = self.version();
                self.ask(
                    "textDocument/formatting",
                    "documentFormattingProvider",
                    params,
                    Asked::Format(version),
                )
            }
            NAVIGATE_BACK => self.navigate(true),
            NAVIGATE_FORWARD => self.navigate(false),
            _ => return None,
        })
    }

    /// Has the server format the file and the save write it once the edits
    /// are in, where the settings ask it of a save; `None` leaves the save
    /// to go ahead at once, as it does for a file no server formats.
    pub(super) fn format_before_save(&mut self) -> Option<EditorOutcome> {
        if !self.format_on_save {
            return None;
        }
        if self.lsp.saving.is_some() {
            return Some(EditorOutcome::Consumed);
        }
        let language = self.language();
        let port = session::with(|session| {
            session.sync(self.document, &language);
            session.serving(self.document, &language).cloned()
        })?;
        if !port.provides("documentFormattingProvider") {
            return None;
        }
        let id = port.request("textDocument/formatting", self.format_params())?;
        self.lsp
            .asked
            .insert((port.id(), id), Asked::FormatAndSave(self.version()));
        self.lsp.saving = Some(Instant::now() + FORMAT_SAVE_WAIT);
        Some(EditorOutcome::Consumed)
    }

    /// When a save waiting on the server's formatting gives up.
    pub(super) fn lsp_next_tick(&self) -> Option<Instant> {
        self.lsp.saving
    }

    /// Writes the file of a save the server has kept waiting too long.
    pub(super) fn lsp_tick(&mut self, now: Instant) -> Option<EditorOutcome> {
        let late = self.lsp.saving.is_some_and(|due| now >= due);
        late.then(|| {
            self.lsp.saving = None;
            self.notice =
                Some(Notice::info("the server took too long to format: saved as it is").unlogged());
            self.write()
        })
    }

    /// Acts on the answer to request `id` of `server`; `None` for a request
    /// this tab did not make.
    pub(super) fn on_lsp_answer(
        &mut self,
        server: ServerId,
        id: u64,
        result: &Result<Value, String>,
    ) -> Option<EditorOutcome> {
        let asked = self.lsp.asked.remove(&(server, id))?;
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                return Some(match asked {
                    Asked::FormatAndSave(_) => {
                        self.lsp.saving = None;
                        self.notice = Some(Notice::info(format!(
                            "not formatted ({error}): saved as it is"
                        )));
                        self.write()
                    }
                    _ => {
                        self.notice = Some(Notice::error(error.clone()));
                        EditorOutcome::Consumed
                    }
                });
            }
        };
        Some(self.answered(server, asked, value))
    }

    /// Acts on the answer to one of this tab's prompts and lists; `None`
    /// for another's.
    pub(super) fn on_lsp_prompt(&mut self, reply: &PromptReply) -> Option<EditorOutcome> {
        let answer = reply.answer.as_deref();
        match reply.tag {
            ASK_RENAME => Some(
                match answer.map(str::trim).filter(|name| !name.is_empty()) {
                    Some(name) => {
                        let mut params = self.here_params();
                        params["newName"] = json!(name);
                        let version = self.version();
                        self.ask(
                            "textDocument/rename",
                            "renameProvider",
                            params,
                            Asked::Rename(version),
                        )
                    }
                    None => EditorOutcome::Consumed,
                },
            ),
            ASK_PLACE => {
                let place = answer.and_then(|answer| {
                    let found = self.lsp.places.iter().find(|(label, _)| label == answer);
                    found.map(|(_, place)| place.clone())
                });
                Some(match place {
                    Some(place) => self.go_to(place, true),
                    None => EditorOutcome::Consumed,
                })
            }
            ASK_FIX => {
                let fix = answer.and_then(|answer| {
                    let at = self
                        .lsp
                        .fixes
                        .iter()
                        .position(|(title, ..)| title == answer)?;
                    Some(self.lsp.fixes.swap_remove(at))
                });
                self.lsp.fixes.clear();
                if let Some((_, server, fix)) = fix {
                    self.take_fix(server, fix);
                }
                Some(EditorOutcome::Consumed)
            }
            _ => None,
        }
    }

    /// Closes the card at the cursor. Whether one was open.
    pub(super) fn close_card(&mut self) -> bool {
        self.lsp.card.take().is_some()
    }

    /// Draws the card at the cursor, hung from `cursor` within `bounds`.
    pub(super) fn draw_card(&self, frame: &mut Frame, cursor: Rect, bounds: Rect, theme: &Theme) {
        let Some(card) = &self.lsp.card else {
            return;
        };
        let color = TextColor::Token(ThemeToken::Foreground);
        let lines = card
            .text
            .lines()
            .map(|line| StyledText::plain(line, color))
            .collect();
        let mut shown = InfoCard::new(lines, cursor);
        if let Some(footer) = &card.footer {
            shown = shown.with_footer(footer.as_str());
        }
        shown.draw(frame, bounds, theme);
    }

    /// Sends `method` with `params` to the server serving the file, which
    /// answers it where it says it provides `capability`, noting what it
    /// was for; says why not where it cannot be asked.
    fn ask(
        &mut self,
        method: &str,
        capability: &str,
        params: Value,
        asked: Asked,
    ) -> EditorOutcome {
        let Some(port) = self.port() else {
            self.notice = Some(Notice::info(NOT_SERVED));
            return EditorOutcome::Consumed;
        };
        if !port.provides(capability) {
            let what = capability.trim_end_matches("Provider");
            self.notice = Some(Notice::info(format!(
                "{} does not answer {what}",
                port.name()
            )));
            return EditorOutcome::Consumed;
        }
        match port.request(method, params) {
            Some(id) => {
                self.lsp.asked.insert((port.id(), id), asked);
            }
            None => self.notice = Some(Notice::info(format!("{} is still starting", port.name()))),
        }
        EditorOutcome::Consumed
    }

    /// Asks for the fixes and refactorings at the cursor, the quick fixes
    /// for the problems on its line first.
    fn ask_fixes(&mut self) -> EditorOutcome {
        let at = self.here();
        let range = protocol::Range { end: at, start: at };
        let problems: Vec<_> = session::with(|session| {
            let file = session.files.get(&self.document)?;
            Some(
                file.problems
                    .iter()
                    .filter(|problem| {
                        problem.range.start.line <= at.line && problem.range.end.line >= at.line
                    })
                    .cloned()
                    .collect(),
            )
        })
        .unwrap_or_default();
        let params = protocol::code_action_params(&self.path, range, &problems);
        let version = self.version();
        self.ask(
            "textDocument/codeAction",
            "codeActionProvider",
            params,
            Asked::Actions(version),
        )
    }

    /// Acts on the answer to `asked`.
    fn answered(&mut self, server: ServerId, asked: Asked, value: &Value) -> EditorOutcome {
        match asked {
            Asked::Actions(version) | Asked::Format(version) | Asked::Rename(version)
                if version != self.version() =>
            {
                self.notice = Some(Notice::info(STALE_EDITS));
                EditorOutcome::Consumed
            }
            Asked::Actions(_) => {
                let fixes = protocol::code_actions(value);
                if fixes.is_empty() {
                    self.notice = Some(Notice::info("no fixes here").unlogged());
                    return EditorOutcome::Consumed;
                }
                self.lsp.fixes = fixes
                    .into_iter()
                    .map(|fix| (fix.title.clone(), server, fix))
                    .collect();
                EditorOutcome::Pick(PickRequest {
                    icons: Vec::new(),
                    items: self
                        .lsp
                        .fixes
                        .iter()
                        .map(|(title, ..)| title.clone())
                        .collect(),
                    label: "Fixes".to_owned(),
                    placeholder: "Filter fixes".to_owned(),
                    placement: PickPlacement::Cursor,
                    tag: ASK_FIX,
                })
            }
            Asked::Command => EditorOutcome::Consumed,
            Asked::Format(_) => {
                let edits = protocol::text_edits(value);
                let said = match edits.is_empty() {
                    true => "already formatted",
                    false => {
                        self.make_edits(&edits);
                        "formatted"
                    }
                };
                self.notice = Some(Notice::info(said).unlogged());
                EditorOutcome::Consumed
            }
            Asked::FormatAndSave(version) => {
                // A save that stopped waiting has written the file already.
                if self.lsp.saving.take().is_none() {
                    return EditorOutcome::Consumed;
                }
                let edits = protocol::text_edits(value);
                if version == self.version() {
                    self.make_edits(&edits);
                }
                self.write()
            }
            Asked::Goto(what) => match protocol::locations(value).as_slice() {
                [] => {
                    self.notice = Some(Notice::info(format!("no {what} found")));
                    EditorOutcome::Consumed
                }
                [only] => {
                    let place = self.place_of(only);
                    self.go_to(place, true)
                }
                found => self.list_places(&format!("{what}s"), found),
            },
            Asked::Hover => {
                match protocol::hover_markdown(value) {
                    Some(text) if !text.trim().is_empty() => {
                        self.lsp.card = Some(Card {
                            footer: None,
                            text: text.trim().to_owned(),
                        });
                    }
                    _ => self.notice = Some(Notice::info("nothing to say about this").unlogged()),
                }
                EditorOutcome::Consumed
            }
            Asked::References => match protocol::locations(value).as_slice() {
                [] => {
                    self.notice = Some(Notice::info("no references found"));
                    EditorOutcome::Consumed
                }
                found => self.list_places("references", found),
            },
            Asked::Rename(_) => {
                let edits = protocol::workspace_edits(value);
                let files = edits.len();
                self.make_file_edits(&edits);
                let said = match files {
                    0 => "nothing to rename".to_owned(),
                    1 => "renamed in 1 file".to_owned(),
                    n => format!("renamed in {n} files, not yet saved"),
                };
                self.notice = Some(Notice::info(said));
                EditorOutcome::Consumed
            }
            Asked::Signature => {
                match protocol::signature_text(value) {
                    Some(text) => {
                        self.lsp.card = Some(Card { footer: None, text });
                    }
                    None => self.notice = Some(Notice::info("no signature here").unlogged()),
                }
                EditorOutcome::Consumed
            }
        }
    }

    /// Makes a fix's edits, and has the server run its command.
    fn take_fix(&mut self, server: ServerId, fix: CodeAction) {
        if let Some(edits) = &fix.edits {
            self.make_file_edits(edits);
        }
        let Some(command) = fix.command else {
            return;
        };
        let Some(port) = session::with(|session| session.port(server)) else {
            return;
        };
        let params = json!({
            "command": command.get("command").cloned().unwrap_or(Value::Null),
            "arguments": command.get("arguments").cloned().unwrap_or(json!([])),
        });
        if let Some(id) = port.request("workspace/executeCommand", params) {
            self.lsp.asked.insert((server, id), Asked::Command);
        }
    }

    /// Makes `edits`, file by file: this tab's own here, another file's by
    /// the window, in the buffer holding it.
    fn make_file_edits(&mut self, edits: &FileEdits) {
        for (path, edits) in edits {
            if edits.is_empty() {
                continue;
            }
            match self.holds(path) {
                true => self.make_edits(edits),
                false => self.lsp.requests.push(EditorRequest::Edit {
                    edits: edits.iter().map(file_edit).collect(),
                    path: path.clone(),
                }),
            }
        }
    }

    /// Makes `edits` to this tab's file as one change.
    fn make_edits(&mut self, edits: &[TextEdit]) {
        let edits: Vec<_> = edits.iter().map(file_edit).collect();
        self.apply_text_edits(&edits);
    }

    /// Goes to `place`, remembering where this left from when `remember`
    /// is set: in this file, the cursor moves; another file is the
    /// window's to show.
    fn go_to(&mut self, place: OpenTarget, remember: bool) -> EditorOutcome {
        let here = self.place_here();
        if remember && here != place {
            JUMPS.with(|jumps| {
                let mut jumps = jumps.borrow_mut();
                jumps.back.push(here);
                if jumps.back.len() > MAX_JUMPS {
                    jumps.back.remove(0);
                }
                jumps.forward.clear();
            });
        }
        if !self.holds(&place.path) {
            return EditorOutcome::OpenPath(place);
        }
        let line = place.line.unwrap_or(1).saturating_sub(1);
        self.move_to(line, place.column.unwrap_or(1).saturating_sub(1));
        EditorOutcome::Consumed
    }

    /// Back over the last jump, or forward again over one gone back over.
    fn navigate(&mut self, back: bool) -> EditorOutcome {
        let taken = JUMPS.with(|jumps| {
            let mut jumps = jumps.borrow_mut();
            match back {
                true => jumps.back.pop(),
                false => jumps.forward.pop(),
            }
        });
        let Some(place) = taken else {
            let said = match back {
                true => "nowhere to go back to",
                false => "nowhere to go forward to",
            };
            self.notice = Some(Notice::info(said).unlogged());
            return EditorOutcome::Consumed;
        };
        let here = self.place_here();
        JUMPS.with(|jumps| {
            let mut jumps = jumps.borrow_mut();
            match back {
                true => jumps.forward.push(here),
                false => jumps.back.push(here),
            }
        });
        self.go_to(place, false)
    }

    /// `found`, the places a list is of, to pick one from, each labelled
    /// with its file, line, column, and the line's text.
    fn list_places(&mut self, what: &str, found: &[Location]) -> EditorOutcome {
        let mut files: HashMap<PathBuf, Vec<String>> = HashMap::new();
        let mut places = Vec::new();
        for (at, location) in found.iter().enumerate() {
            let place = self.place_of(location);
            let lines = files
                .entry(location.path.clone())
                .or_insert_with(|| self.lines_of(&location.path));
            let line = place.line.unwrap_or(1);
            let text = lines.get(line - 1).map_or("", |text| text.trim());
            let shown = winter_model::path::abbreviate_home(&location.path);
            let column = place.column.unwrap_or(1);
            let mut label = format!("{shown}:{line}:{column}  {text}");
            // Two places reading the same are told apart by their order.
            if places.iter().any(|(known, _)| *known == label) {
                label = format!("{label} ({})", at + 1);
            }
            places.push((label, place));
        }
        self.lsp.places = places;
        EditorOutcome::Pick(PickRequest {
            icons: Vec::new(),
            items: self
                .lsp
                .places
                .iter()
                .map(|(label, _)| label.clone())
                .collect(),
            label: format!("{} {what}", self.lsp.places.len()),
            placeholder: "Filter by path".to_owned(),
            // At the bottom of the pane, so the code the places are about
            // stays in sight above the list.
            placement: PickPlacement::EditorBottom,
            tag: ASK_PLACE,
        })
    }

    /// The place `location` names, its column in chars.
    fn place_of(&self, location: &Location) -> OpenTarget {
        let lines = self.lines_of(&location.path);
        let (line, column) = protocol::place_of(&lines, location.range.start);
        OpenTarget {
            column: Some(column + 1),
            line: Some(line + 1),
            path: location.path.clone(),
        }
    }

    /// Where the cursor is, as a place to come back to.
    fn place_here(&self) -> OpenTarget {
        let (line, column) = session::with(|session| {
            let cursor = session.editor.view(self.view_id())?.cursor();
            let at = session
                .editor
                .document(self.document)?
                .char_to_position(cursor)?;
            Some((at.line, at.column))
        })
        .unwrap_or_default();
        OpenTarget {
            column: Some(column + 1),
            line: Some(line + 1),
            path: self.path.clone(),
        }
    }

    /// A file's lines: this tab's as edited, another's as on disk.
    fn lines_of(&self, path: &Path) -> Vec<String> {
        match self.holds(path) {
            true => session::with(|session| session.lines(self.document)),
            false => winter_model::text_file::load(path)
                .map(|loaded| loaded.lines)
                .unwrap_or_default(),
        }
    }

    /// Whether `path` is this tab's file.
    fn holds(&self, path: &Path) -> bool {
        use winter_model::editor::Editor;
        self.holds_file(path)
    }

    /// The server serving the file, its text handed over first.
    fn port(&self) -> Option<Port> {
        let language = self.language();
        session::with(|session| {
            session.sync(self.document, &language);
            session.serving(self.document, &language).cloned()
        })
    }

    /// The file and the cursor's place in it, as a request about the
    /// cursor is sent with.
    fn here_params(&self) -> Value {
        protocol::at_params(&self.path, self.here())
    }

    /// The cursor's place, as a server counts it.
    fn here(&self) -> protocol::Position {
        let at = session::with(|session| {
            let cursor = session.editor.view(self.view_id())?.cursor();
            session
                .editor
                .document(self.document)?
                .char_to_utf16_position(cursor)
        });
        let Utf16Position { column, line } = at.unwrap_or(Utf16Position { column: 0, line: 0 });
        protocol::Position {
            character: u32::try_from(column).unwrap_or(u32::MAX),
            line: u32::try_from(line).unwrap_or(u32::MAX),
        }
    }

    /// The file, and what asks to format it with its own indentation: tabs
    /// where it indents with them.
    fn format_params(&self) -> Value {
        let lines = session::with(|session| session.lines(self.document));
        let tabs = lines.iter().any(|line| line.starts_with('\t'));
        protocol::format_params(&self.path, winter_model::paint::TAB_WIDTH, !tabs)
    }

    /// The word the cursor is on, what a rename starts from.
    fn word_here(&self) -> String {
        session::with(|session| {
            let cursor = session.editor.view(self.view_id())?.cursor();
            let text = session.editor.document(self.document)?;
            let at = text.char_to_position(cursor)?;
            let line: Vec<char> = text.line_text(at.line)?.chars().collect();
            let word = |c: &char| c.is_alphanumeric() || *c == '_';
            let column = at.column.min(line.len());
            let start = line[..column].iter().rev().take_while(|c| word(c)).count();
            let end = line[column..].iter().take_while(|c| word(c)).count();
            Some(line[column - start..column + end].iter().collect())
        })
        .unwrap_or_default()
    }

    /// The text's version, which edits worked out from it were made for.
    fn version(&self) -> u64 {
        session::with(|session| session.editor.document_version(self.document)).unwrap_or(0)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;

    use winter_model::editor::Page;

    use super::*;

    /// A tab on a Rust file of its own holding `text`, its server a fake
    /// that answers everything, and what is sent to that server.
    fn served(tag: &str, text: &str) -> (TextEditor, Receiver<Value>, ServerId) {
        let dir =
            std::env::temp_dir().join(format!("winter-requests-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let path = dir.join("main.rs");
        std::fs::write(&path, text).expect("written");
        let every = json!({
            "definitionProvider": true,
            "documentFormattingProvider": true,
            "renameProvider": true,
        });
        let (port, sent) = Port::fake("rust", &dir, every);
        let server = port.id();
        let mut editor = TextEditor::new(path, None).expect("opens");
        editor.connect_lsp(1, &[port]);
        (editor, sent, server)
    }

    /// The id of the last request sent with `method`.
    fn asked(sent: &Receiver<Value>, method: &str) -> u64 {
        sent.try_iter()
            .filter(|message| message["method"] == method)
            .filter_map(|message| message["id"].as_u64())
            .last()
            .expect("the request went out")
    }

    /// A location in `path` at `line` and `character`.
    fn location(path: &Path, line: u32, character: u32) -> Value {
        let at = json!({"line": line, "character": character});
        json!({"uri": protocol::uri_of(path), "range": {"start": at, "end": at}})
    }

    #[test]
    fn test_a_definition_in_the_file_moves_the_cursor_and_one_elsewhere_opens_it() {
        let (mut editor, sent, server) = served("goto", "fn a() {}\nfn b() { a() }\n");
        editor.run_lsp_command(LSP_DEFINITION);
        let id = asked(&sent, "textDocument/definition");
        let here = location(&editor.path, 0, 3);
        let outcome = editor.on_lsp_answer(server, id, &Ok(here));
        assert_eq!(outcome, Some(EditorOutcome::Consumed));
        assert_eq!(
            editor.status().segments.first().map(String::as_str),
            Some("1:4")
        );
        editor.run_lsp_command(LSP_DEFINITION);
        let id = asked(&sent, "textDocument/definition");
        let elsewhere = std::env::temp_dir().join("elsewhere.rs");
        let outcome = editor.on_lsp_answer(server, id, &Ok(location(&elsewhere, 4, 0)));
        let Some(EditorOutcome::OpenPath(target)) = outcome else {
            panic!("the window opens the other file: {outcome:?}");
        };
        assert_eq!((target.path, target.line), (elsewhere, Some(5)));
        assert_eq!(
            editor.on_lsp_answer(server, id, &Ok(Value::Null)),
            None,
            "answered once"
        );
    }

    #[test]
    fn test_a_rename_starts_from_the_word_at_the_cursor() {
        let (mut editor, _, _) = served("rename", "let count = 1;\n");
        editor.move_to(0, 6);
        let Some(EditorOutcome::Prompt(prompt)) = editor.run_lsp_command(LSP_RENAME) else {
            panic!("asks for the new name");
        };
        assert_eq!(prompt.initial, "count");
    }

    #[test]
    fn test_a_save_waits_on_formatting_and_writes_the_formatted_text() {
        let (mut editor, sent, server) = served("format", "fn  a() {}\n");
        editor.format_on_save = true;
        assert_eq!(editor.format_before_save(), Some(EditorOutcome::Consumed));
        let id = asked(&sent, "textDocument/formatting");
        let edit = json!([{
            "range": {"start": {"line": 0, "character": 2}, "end": {"line": 0, "character": 4}},
            "newText": " ",
        }]);
        let outcome = editor.on_lsp_answer(server, id, &Ok(edit));
        assert!(
            matches!(outcome, Some(EditorOutcome::Wrote(_))),
            "{outcome:?}"
        );
        let written = std::fs::read_to_string(&editor.path).expect("written");
        assert_eq!(written, "fn a() {}\n");
    }
}
