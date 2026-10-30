//! GitHub Copilot's suggestions in the editor, as magic-vscode has them: the
//! text Copilot expects next drawn dim at the cursor as you type (ghost
//! text), and after a change elsewhere the edit it expects next, in a card.
//!
//! While ghost text shows, `Tab` (or `C-RET`) takes all of it, `M-f`,
//! `M-<right>`, or `C-,` its next word, `C-.` its next line, `M-n` and `M-p`
//! step through what else Copilot offered, and `C-g` hides it (as `Esc`
//! does on its way back to Normal mode). Typing what it shows keeps it
//! showing. `C-\` asks for ghost text where none came by itself. A next
//! edit is made with `Tab` and left with `Esc`.
//!
//! The server is the window's to start (see `window/editor/copilot.rs`);
//! here it is the one server reading every file, handed each document as it
//! changes.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config::CopilotConfig;
use winter_lsp::protocol::{self, Range, TextEdit};
use winter_lsp::{Event, Incoming, ServerId};
use crate::model::editor::{EditorOutcome, InfoCard};
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::paint::{Style, TAB_WIDTH};

use super::lsp::hand_over;
use super::{Cell, TextEditor};

// ========================================================================
// Constants
// ========================================================================

/// How long after an edit Copilot is asked, so a burst of typing is one
/// question: Zed's and magic-vscode's 75ms.
const DEBOUNCE: Duration = Duration::from_millis(75);

/// The trigger kinds a completion request names: asked for (`C-\`), or
/// while typing.
const INVOKED: u64 = 1;
const AUTOMATIC: u64 = 2;

/// The most lines a next edit's card shows of each side.
const CARD_LINES: usize = 8;

// ========================================================================
// Data Structures
// ========================================================================

/// Copilot as one editor sees it: the request in flight, the one waiting to
/// be made, and what is on screen.
#[derive(Debug, Default)]
pub(super) struct Pilot {
    /// The request in flight: its server and id, and what it asks.
    asked: Option<(ServerId, u64, Ask)>,
    config: Option<CopilotConfig>,
    /// When to ask next, and whether for ghost text or a next edit.
    due: Option<(Instant, Want)>,
    /// The file the server was last told has the focus.
    focused: Option<PathBuf>,
    ghost: Option<Ghost>,
    next_edit: Option<NextEdit>,
}

/// What a request asks for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ask {
    /// Ghost text on row `row`; `invoked` where `C-\` asked for it.
    Ghost { invoked: bool, row: usize },
    NextEdit,
    /// Nothing to act on: an acceptance told to the server.
    Told,
}

/// What to ask for when the time comes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Want {
    Ghost { invoked: bool },
    NextEdit,
}

/// The ghost text offered: every suggestion, which is shown, and the row it
/// is for.
#[derive(Clone, Debug, PartialEq)]
struct Ghost {
    at: usize,
    items: Vec<Item>,
    row: usize,
    /// Which suggestions the server has been told were shown.
    told: Vec<usize>,
}

/// One suggestion: what replaces which range, and the command that tells
/// the server it was taken.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    command: Option<Value>,
    insert: String,
    range: Range,
}

/// The edit Copilot expects next.
#[derive(Clone, Debug, PartialEq)]
struct NextEdit {
    command: Option<Value>,
    range: Range,
    /// The buffer's revision it was offered at: an edit since makes it stale.
    revision: usize,
    text: String,
}

// ========================================================================
// TextEditor: Copilot
// ========================================================================

impl TextEditor {
    /// Take the settings Copilot's suggestions follow.
    pub(super) fn take_copilot_config(&mut self, config: &CopilotConfig) {
        if self.pilot.config.as_ref() != Some(config) {
            self.pilot.config = Some(config.clone());
            if !config.enable || !config.completions {
                self.pilot.ghost = None;
            }
            if !config.enable || !config.next_edit {
                self.pilot.next_edit = None;
            }
        }
    }

    /// Whether Copilot is on and running, with ghost text (`ghost`) or next
    /// edits wanted.
    fn copilot_wants(&self, ghost: bool) -> bool {
        let Some(config) = self.pilot.config.as_ref().filter(|config| config.enable) else {
            return false;
        };
        let wanted = match ghost {
            true => config.completions,
            false => config.next_edit,
        };
        wanted && self.copilot_port().is_some() && crate::remote::parse(&self.doc().path).is_none()
    }

    /// After a key: an edit asks Copilot again once typing pauses, ghost text
    /// while typing and a next edit otherwise; moving off the ghost text's
    /// row puts it away.
    pub(super) fn copilot_after_key(&mut self, revision: usize) {
        let edited = self.buffer().revision() != revision;
        if let Some(ghost) = &self.pilot.ghost {
            if !self.modal.is_typing() || ghost.row != self.buffer().row() || self.ghost_text().is_none() {
                self.pilot.ghost = None;
            }
        }
        if !edited {
            return;
        }
        if self.pilot.next_edit.as_ref().is_some_and(|edit| edit.revision != self.buffer().revision()) {
            self.pilot.next_edit = None;
        }
        let want = match self.modal.is_typing() && self.copilot_wants(true) {
            true => Want::Ghost { invoked: false },
            false => Want::NextEdit,
        };
        if want == Want::NextEdit && !self.copilot_wants(false) {
            return;
        }
        self.pilot.due = Some((Instant::now() + DEBOUNCE, want));
    }

    /// When Copilot is next to be asked.
    pub(super) fn copilot_next_tick(&self) -> Option<Instant> {
        self.pilot.due.map(|(at, _)| at)
    }

    /// Ask Copilot what it is due to be asked.
    pub(super) fn copilot_tick(&mut self, now: Instant) {
        let Some((at, want)) = self.pilot.due else {
            return;
        };
        if at > now {
            return;
        }
        self.pilot.due = None;
        self.ask_copilot(want);
    }

    /// Hand Copilot every document as it now reads.
    fn copilot_sync(&mut self) {
        let Some(port) = self.copilot_port().cloned() else {
            return;
        };
        for doc in &mut self.docs {
            if doc.unread || crate::remote::parse(&doc.path).is_some() {
                continue;
            }
            doc.buffer.catch_up();
            let synced = hand_over(&port, doc, doc.pilot_synced());
            doc.set_pilot_synced(synced);
        }
        let path = self.doc().path.clone();
        if self.pilot.focused.as_ref() != Some(&path) {
            port.notify("textDocument/didFocus", json!({"textDocument": {"uri": protocol::uri_of(&path)}}));
            self.pilot.focused = Some(path);
        }
    }

    /// Ask Copilot for ghost text at the cursor, or the next edit.
    fn ask_copilot(&mut self, want: Want) {
        let ghost = matches!(want, Want::Ghost { .. });
        if !self.copilot_wants(ghost) {
            return;
        }
        self.copilot_sync();
        let Some(port) = self.copilot_port().cloned() else {
            return;
        };
        let Some(version) = self.doc().pilot_synced().map(|synced| synced.version) else {
            return;
        };
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let at = protocol::position_in(self.buffer().lines(), row, col);
        let document = json!({"uri": protocol::uri_of(&self.doc().path), "version": version});
        let position = json!({"line": at.line, "character": at.character});
        let (method, params, ask) = match want {
            Want::Ghost { invoked } => {
                let spaces = !self.buffer().lines().iter().any(|line| line.starts_with('\t'));
                (
                    "textDocument/inlineCompletion",
                    json!({
                        "textDocument": document,
                        "position": position,
                        "context": {"triggerKind": if invoked { INVOKED } else { AUTOMATIC }},
                        "formattingOptions": {"tabSize": TAB_WIDTH, "insertSpaces": spaces},
                    }),
                    Ask::Ghost { invoked, row },
                )
            }
            Want::NextEdit => (
                "textDocument/copilotInlineEdit",
                json!({"textDocument": document, "position": position}),
                Ask::NextEdit,
            ),
        };
        if let Some(id) = port.request(method, params) {
            self.pilot.asked = Some((port.id(), id, ask));
        }
    }

    /// An answer from Copilot, where it is one this editor asked for.
    pub(super) fn copilot_answer(&mut self, incoming: &Incoming) -> Option<EditorOutcome> {
        let Event::Answer { id, result } = &incoming.event else {
            return None;
        };
        let (server, asked, ask) = self.pilot.asked?;
        if (server, asked) != (incoming.server, *id) {
            return None;
        }
        self.pilot.asked = None;
        let Ok(value) = result else {
            return Some(EditorOutcome::Consumed);
        };
        match ask {
            Ask::Ghost { invoked, row } => self.take_ghost(value, row, invoked),
            Ask::NextEdit => self.take_next_edit(value),
            Ask::Told => {}
        }
        Some(EditorOutcome::Consumed)
    }

    /// Take the ghost text Copilot offered on row `row`, or ask for the next
    /// edit where it offered none.
    fn take_ghost(&mut self, value: &Value, row: usize, invoked: bool) {
        if !self.modal.is_typing() || self.buffer().row() != row {
            return;
        }
        let items: Vec<Item> = value
            .get("items")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(item_of).collect())
            .unwrap_or_default();
        self.pilot.ghost = (!items.is_empty()).then(|| Ghost {
            at: 0,
            items,
            row,
            told: Vec::new(),
        });
        if self.ghost_text().is_none() {
            self.pilot.ghost = None;
            if invoked {
                self.modal.report(crate::model::editor::Notice::info("Copilot: nothing to suggest here").unlogged());
            } else if self.copilot_wants(false) {
                self.ask_copilot(Want::NextEdit);
            }
            return;
        }
        self.tell_shown();
    }

    /// Take the next edit Copilot offered, for the version it was asked at.
    fn take_next_edit(&mut self, value: &Value) {
        let Some(edit) = value.get("edits").and_then(Value::as_array).and_then(|edits| edits.first()) else {
            return;
        };
        let version = edit.pointer("/textDocument/version").and_then(Value::as_i64);
        let current = self.doc().pilot_synced().map(|synced| synced.version as i64);
        let (Some(text), Some(range)) = (edit.get("text").and_then(Value::as_str), edit.get("range").and_then(range_of)) else {
            return;
        };
        if version.is_some() && version != current {
            return;
        }
        self.pilot.next_edit = Some(NextEdit {
            command: edit.get("command").cloned(),
            range,
            revision: self.buffer().revision(),
            text: text.to_string(),
        });
        if let Some(port) = self.copilot_port() {
            let arguments = edit.pointer("/command/arguments").cloned().unwrap_or(json!([]));
            port.notify("textDocument/didShowInlineEdit", json!({"item": {"command": {"arguments": arguments}}}));
        }
    }

    /// Tell the server the suggestion on screen was shown, once each.
    fn tell_shown(&mut self) {
        let Some(port) = self.copilot_port().cloned() else {
            return;
        };
        let Some(ghost) = self.pilot.ghost.as_mut() else {
            return;
        };
        if ghost.told.contains(&ghost.at) {
            return;
        }
        ghost.told.push(ghost.at);
        let arguments = ghost.items[ghost.at].command.as_ref().and_then(|command| command.get("arguments").cloned());
        port.notify("textDocument/didShowCompletion", json!({"item": {"command": {"arguments": arguments.unwrap_or(json!([]))}}}));
    }

    /// The ghost text to draw at the cursor: what the suggestion shown puts
    /// in past what is already typed, less any of the line's own text after
    /// the cursor it ends with. `None` where none shows, or where what is
    /// typed has parted from it.
    pub(super) fn ghost_text(&self) -> Option<String> {
        let ghost = self.pilot.ghost.as_ref()?;
        let item = ghost.items.get(ghost.at)?;
        let row = self.buffer().row();
        if row != ghost.row || item.range.start.line as usize != row || item.range.end.line as usize != row {
            return None;
        }
        let lines = self.buffer().lines();
        let line: Vec<char> = lines.get(row)?.chars().collect();
        let col = self.buffer().col().min(line.len());
        let (_, start) = protocol::place_of(lines, item.range.start);
        let (_, end) = protocol::place_of(lines, item.range.end);
        if start > col {
            return None;
        }
        let typed: String = line[start..col].iter().collect();
        let rest = item.insert.strip_prefix(&typed)?;
        let tail: String = line[col..end.max(col).min(line.len())].iter().collect();
        let rest = rest.strip_suffix(&tail).unwrap_or(rest);
        (!rest.is_empty()).then(|| rest.to_string())
    }

    /// A key Copilot's suggestions give a meaning of their own: what it did,
    /// or `None` for a key left to the rest.
    pub(super) fn copilot_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        let chord = key.chord();
        if chord == (Mods::CTRL, KeyCode::Char('\\')) && self.copilot_wants(true) {
            self.ask_copilot(Want::Ghost { invoked: true });
            return Some(EditorOutcome::Consumed);
        }
        if self.ghost_text().is_some() && self.modal.is_typing() {
            return match chord {
                (Mods::NONE, KeyCode::Tab) | (Mods::CTRL, KeyCode::Enter) => Some(self.take_whole_ghost()),
                (Mods::ALT, KeyCode::Char('f') | KeyCode::Right) | (Mods::CTRL, KeyCode::Char(',')) => {
                    Some(self.take_ghost_part(false))
                }
                (Mods::CTRL, KeyCode::Char('.')) => Some(self.take_ghost_part(true)),
                (Mods::ALT, KeyCode::Char('n')) => Some(self.step_ghost(true)),
                (Mods::ALT, KeyCode::Char('p')) => Some(self.step_ghost(false)),
                (Mods::CTRL, KeyCode::Char('g')) => {
                    self.pilot.ghost = None;
                    Some(EditorOutcome::Consumed)
                }
                (Mods::NONE, KeyCode::Escape) => {
                    self.pilot.ghost = None;
                    None
                }
                _ => None,
            };
        }
        if self.pilot.next_edit.is_some() {
            return match chord {
                (Mods::NONE, KeyCode::Tab) => Some(self.take_next_edit_now()),
                (Mods::NONE, KeyCode::Escape) => {
                    self.pilot.next_edit = None;
                    None
                }
                (Mods::CTRL, KeyCode::Char('g')) => {
                    self.pilot.next_edit = None;
                    Some(EditorOutcome::Consumed)
                }
                _ => None,
            };
        }
        None
    }

    /// Take the whole suggestion shown: what the ghost text shows put in at
    /// the cursor (the line's own text after it being what the suggestion
    /// ends with), the cursor after it, and the server told.
    fn take_whole_ghost(&mut self) -> EditorOutcome {
        let Some(rest) = self.ghost_text() else {
            return EditorOutcome::Consumed;
        };
        let command = self.pilot.ghost.take().and_then(|ghost| ghost.items[ghost.at].command.clone());
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let at = protocol::position_in(self.buffer().lines(), row, col);
        let index = self.at.min(self.docs.len() - 1);
        self.apply_to_buffer(
            index,
            &[TextEdit {
                new_text: rest.clone(),
                range: Range { end: at, start: at },
            }],
        );
        let (end_row, end_col) = end_of(row, col, &rest);
        self.buffer_mut().move_to_typing(end_row, end_col);
        self.tell_taken(command.as_ref());
        EditorOutcome::Consumed
    }

    /// Take the ghost text's next word (`line` unset) or the rest of its
    /// line, the line after where it is at a line's end, typed in as if
    /// typed: what is left keeps showing.
    fn take_ghost_part(&mut self, line: bool) -> EditorOutcome {
        let Some(ghost) = self.ghost_text() else {
            return EditorOutcome::Consumed;
        };
        let chars: Vec<char> = ghost.chars().collect();
        let end = match line {
            true => match chars.first() {
                Some('\n') => 1 + chars[1..].iter().take_while(|c| **c != '\n').count(),
                _ => chars.iter().take_while(|c| **c != '\n').count(),
            },
            false => {
                // Spaces before the word, then the word, as Emacs' `M-f`.
                let spaces = chars.iter().take_while(|c| c.is_whitespace()).count();
                let word = chars[spaces..].iter().take_while(|c| !c.is_whitespace()).count();
                (spaces + word).max(1)
            }
        };
        let piece: String = chars[..end.min(chars.len())].iter().collect();
        let row = self.buffer().row();
        let outcome = self.paste_text(piece.clone());
        // The suggestion stays for the row the text now ends on.
        if let Some(ghost) = self.pilot.ghost.as_mut() {
            if piece.contains('\n') {
                ghost.row = row + piece.matches('\n').count();
            }
        }
        if self.ghost_text().is_none() {
            let command = self.pilot.ghost.take().and_then(|ghost| ghost.items[ghost.at].command.clone());
            self.tell_taken(command.as_ref());
        }
        outcome
    }

    /// The next suggestion Copilot offered, or the one before, round.
    fn step_ghost(&mut self, forward: bool) -> EditorOutcome {
        if let Some(ghost) = self.pilot.ghost.as_mut() {
            let count = ghost.items.len();
            ghost.at = match forward {
                true => (ghost.at + 1) % count,
                false => (ghost.at + count - 1) % count,
            };
        }
        self.tell_shown();
        EditorOutcome::Consumed
    }

    /// Make the next edit Copilot expects, the cursor after it.
    fn take_next_edit_now(&mut self) -> EditorOutcome {
        let Some(edit) = self.pilot.next_edit.take() else {
            return EditorOutcome::Consumed;
        };
        if edit.revision != self.buffer().revision() {
            return EditorOutcome::Consumed;
        }
        let at = self.at.min(self.docs.len() - 1);
        self.apply_to_buffer(
            at,
            &[TextEdit {
                new_text: edit.text.clone(),
                range: edit.range,
            }],
        );
        let (row, col) = protocol::place_of(self.buffer().lines(), edit.range.start);
        let (end_row, end_col) = end_of(row, col, &edit.text);
        match self.modal.is_typing() {
            true => self.buffer_mut().move_to_typing(end_row, end_col),
            false => self.buffer_mut().move_to(end_row, end_col.saturating_sub(1)),
        }
        self.tell_taken(edit.command.as_ref());
        EditorOutcome::Consumed
    }

    /// Tell the server a suggestion was taken, through the command it gave
    /// with it.
    fn tell_taken(&mut self, command: Option<&Value>) {
        let (Some(command), Some(port)) = (command, self.copilot_port().cloned()) else {
            return;
        };
        let params = json!({
            "command": command.get("command").cloned().unwrap_or(Value::Null),
            "arguments": command.get("arguments").cloned().unwrap_or(json!([])),
        });
        if let Some(id) = port.request("workspace/executeCommand", params) {
            self.pilot.asked = Some((port.id(), id, Ask::Told));
        }
    }

    /// The ghost text's first line as cells for line `index`, and where in
    /// the line's cells it goes: the cursor's.
    pub(super) fn ghost_cells(&self, index: usize) -> Option<(usize, Vec<Cell>)> {
        if index != self.buffer().row() || !self.modal.is_typing() {
            return None;
        }
        let ghost = self.ghost_text()?;
        let first = ghost.split('\n').next().unwrap_or_default().replace('\t', &" ".repeat(TAB_WIDTH));
        let cells = first.chars().map(|c| (c, Style::Dim, None, false)).collect();
        let col = self.buffer().col();
        let at = self.painted_cells_before(index, col);
        Some((at, cells))
    }

    /// The card under the cursor that shows Copilot's suggestion: the lines
    /// of the ghost text past its first, or the next edit expected.
    pub(super) fn copilot_card(&self) -> Option<InfoCard> {
        if self.modal.is_typing() {
            if let Some(ghost) = self.ghost_text() {
                let rest: Vec<&str> = ghost.split('\n').skip(1).collect();
                return (!rest.is_empty()).then(|| InfoCard {
                    cell: None,
                    footer: None,
                    text: rest.join("\n"),
                });
            }
        }
        self.next_edit_card()
    }

    /// The card a next edit shows in: what it takes out and puts in.
    fn next_edit_card(&self) -> Option<InfoCard> {
        let edit = self.pilot.next_edit.as_ref()?;
        let lines = self.buffer().lines();
        let (start_row, start_col) = protocol::place_of(lines, edit.range.start);
        let (end_row, end_col) = protocol::place_of(lines, edit.range.end);
        let taken: Vec<String> = match start_row == end_row {
            true => vec![lines.get(start_row)?.chars().skip(start_col).take(end_col.saturating_sub(start_col)).collect()],
            false => lines.get(start_row..=end_row.min(lines.len().saturating_sub(1)))?.to_vec(),
        };
        let mut text = format!("Copilot: line {} (Tab to make it, Esc to leave it)", start_row + 1);
        for line in taken.iter().filter(|line| !line.is_empty()).take(CARD_LINES) {
            text.push_str(&format!("\n- {line}"));
        }
        for line in edit.text.lines().take(CARD_LINES) {
            text.push_str(&format!("\n+ {line}"));
        }
        Some(InfoCard {
            cell: None,
            footer: None,
            text,
        })
    }

    /// What the status bar says of Copilot: which of its suggestions shows.
    pub(super) fn copilot_status(&self) -> Option<String> {
        let ghost = self.pilot.ghost.as_ref().filter(|_| self.ghost_text().is_some())?;
        Some(match ghost.items.len() {
            1 => "Copilot".to_string(),
            n => format!("Copilot {}/{n}", ghost.at + 1),
        })
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Where text put in at `row`, `col` ends.
fn end_of(row: usize, col: usize, text: &str) -> (usize, usize) {
    let pieces: Vec<&str> = text.split('\n').collect();
    let last = pieces.last().map_or(0, |piece| piece.chars().count());
    match pieces.len() {
        1 => (row, col + last),
        n => (row + n - 1, last),
    }
}

/// One suggestion of a completion answer.
fn item_of(value: &Value) -> Option<Item> {
    Some(Item {
        command: value.get("command").cloned(),
        insert: value.get("insertText")?.as_str()?.to_string(),
        range: range_of(value.get("range")?)?,
    })
}

/// A range from the wire.
fn range_of(value: &Value) -> Option<Range> {
    let at = |key: &str| -> Option<protocol::Position> {
        let point = value.get(key)?;
        Some(protocol::Position {
            character: point.get("character")?.as_u64()? as u32,
            line: point.get("line")?.as_u64()? as u32,
        })
    };
    Some(Range {
        end: at("end")?,
        start: at("start")?,
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::editor::Editor;
    use crate::model::paint::row_text;

    /// An editor over `text` in Insert mode at the end of its first line,
    /// Copilot having offered `insert` over the first `end` characters of
    /// it. `tag` names the file, so tests run side by side.
    fn offered(tag: &str, text: &str, insert: &str, end: u32) -> (TextEditor, PathBuf) {
        let path = std::env::temp_dir().join(format!("winter-copilot-{tag}-{}.txt", std::process::id()));
        std::fs::write(&path, text).expect("temp file");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        editor.on_key(&key(KeyCode::Char('A'), false, false));
        let range = Range {
            end: protocol::Position { character: end, line: 0 },
            start: protocol::Position { character: 0, line: 0 },
        };
        editor.pilot.ghost = Some(Ghost {
            at: 0,
            items: vec![Item {
                command: None,
                insert: insert.to_string(),
                range,
            }],
            row: 0,
            told: Vec::new(),
        });
        (editor, path)
    }

    fn key(code: KeyCode, ctrl: bool, alt: bool) -> Key {
        Key {
            alt,
            code,
            ctrl,
            shift: false,
            sup: false,
        }
    }

    #[test]
    fn test_the_ghost_text_is_what_the_suggestion_adds_past_what_is_typed() {
        let (mut editor, path) = offered("typed", "    fo", "    foo(bar)", 6);
        assert_eq!(editor.ghost_text().as_deref(), Some("o(bar)"));
        editor.on_key(&key(KeyCode::Char('o'), false, false));
        assert_eq!(editor.ghost_text().as_deref(), Some("(bar)"), "typing what it shows keeps it");
        editor.on_key(&key(KeyCode::Char('x'), false, false));
        assert_eq!(editor.ghost_text(), None, "typing past it puts it away");
        assert!(editor.pilot.ghost.is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_the_ghost_text_leaves_out_the_line_s_own_text_after_the_cursor() {
        let (mut editor, path) = offered("tail", "foo()", "foo(x, y)", 5);
        editor.on_key(&key(KeyCode::Left, false, false));
        assert_eq!(editor.ghost_text().as_deref(), Some("x, y"));
        let painted = editor.content(4, 40, false);
        let first = row_text(&painted.rows[0]);
        assert!(first.contains("foo(x, y)"), "drawn at the cursor, the `)` after it: {first}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_a_word_then_the_rest_of_a_suggestion_are_taken() {
        let (mut editor, path) = offered("take", "let", "let total = sum(xs);\nprint(total)", 3);
        editor.on_key(&key(KeyCode::Char('f'), false, true));
        assert_eq!(editor.buffer().lines()[0], "let total");
        assert_eq!(editor.ghost_text().as_deref(), Some(" = sum(xs);\nprint(total)"));
        editor.on_key(&key(KeyCode::Tab, false, false));
        assert_eq!(editor.buffer().lines(), ["let total = sum(xs);", "print(total)"]);
        assert_eq!((editor.buffer().row(), editor.buffer().col()), (1, 12), "after what went in");
        assert!(editor.pilot.ghost.is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_c_g_hides_the_ghost_text_and_tab_then_indents_as_ever() {
        let (mut editor, path) = offered("hide", "x", "xyz", 1);
        editor.on_key(&key(KeyCode::Char('g'), true, false));
        assert_eq!(editor.ghost_text(), None);
        assert_eq!(editor.buffer().lines()[0], "x", "C-g took nothing");
        let _ = std::fs::remove_file(path);
    }
}
