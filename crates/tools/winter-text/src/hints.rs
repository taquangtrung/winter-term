//! Inlay hints and code lenses: what a language server says of the code
//! that the text does not, a binding's type or a call's parameter names in
//! the line where it applies, and a lens (`3 references`) after the last
//! character of the line it is about. They are asked for once typing
//! pauses, of the whole file, and laid in as Viento's inlays, which edits
//! carry along until the next answer.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use viento::{Inlay, Utf16Position};
use winter_lsp::protocol;
use winter_lsp::Port;

use super::{session, TextEditor};

// ========================================================================
// Constants
// ========================================================================

/// How long typing pauses before the hints are asked for again.
const PAUSE: Duration = Duration::from_millis(300);

/// What a server says it can do, for inlay hints and for lenses.
const HINTS: &str = "inlayHintProvider";
const LENSES: &str = "codeLensProvider";

/// The owners of the inlays laid in for each.
const HINT_INLAYS: &str = "hints";
const LENS_INLAYS: &str = "lenses";

/// What sets a lens apart from the line it follows.
const LENS_GAP: &str = "  ";

// ========================================================================
// Data Structures
// ========================================================================

/// The requests a tab sent for the hints and lenses of the text at a
/// version.
pub(super) struct HintAsks {
    /// Each request's id, and whether it is the hints' rather than the
    /// lenses'.
    pub(super) ids: Vec<(u64, bool)>,
    /// The server asked.
    pub(super) port: Port,
    /// The text's version asked about.
    pub(super) version: u64,
}

/// One tab's hints and lenses: when to ask, and what came back.
#[derive(Debug, Default)]
pub(super) struct Hints {
    /// The text's version last asked about, so it is asked once.
    asked: Option<u64>,
    /// When to ask, once typing has paused.
    due: Option<Instant>,
    /// The lenses answered, each the line it is about and its title once
    /// known, for the text at the version asked about.
    lenses: Vec<(usize, Option<String>)>,
}

// ========================================================================
// TextEditor: hints and lenses
// ========================================================================

impl TextEditor {
    /// When the hints are next to be asked for.
    pub(super) fn hints_next_tick(&self) -> Option<Instant> {
        self.hints.due
    }

    /// Asks for the hints and lenses where the text changed since they were
    /// last asked about and typing has paused, of a server that has them.
    /// The server and the version asked about, for the answers to be told
    /// apart by.
    pub(super) fn hints_tick(&mut self, now: Instant) -> Option<HintAsks> {
        let version = self.version();
        if self.hints.asked == Some(version) {
            return None;
        }
        let port = self
            .port()
            .filter(|port| port.provides(HINTS) || port.provides(LENSES))?;
        match self.hints.due {
            None => {
                self.hints.due = Some(now + PAUSE);
                return None;
            }
            Some(due) if due > now => return None,
            Some(_) => {}
        }
        self.hints.due = None;
        self.hints.asked = Some(version);
        let lines = session::with(|session| session.lines(self.document).len());
        let document = json!({"uri": protocol::uri_of(&self.path)});
        let mut ids = Vec::new();
        if port.provides(HINTS) {
            let whole = json!({
                "start": {"line": 0, "character": 0},
                "end": {"line": lines, "character": 0},
            });
            let params = json!({"textDocument": document, "range": whole});
            if let Some(id) = port.request("textDocument/inlayHint", params) {
                ids.push((id, true));
            }
        }
        if port.provides(LENSES) {
            let params = json!({"textDocument": document});
            if let Some(id) = port.request("textDocument/codeLens", params) {
                ids.push((id, false));
            }
        }
        Some(HintAsks { ids, port, version })
    }

    /// Lays in the hints a server answered about the text at `version`.
    pub(super) fn take_hints(&mut self, version: u64, answer: &Value) {
        if version != self.version() {
            return;
        }
        session::with(|session| {
            let Some(text) = session.editor.document(self.document) else {
                return;
            };
            let inlays = answer
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|hint| {
                    let at = offset_of(text, hint.get("position")?)?;
                    Some(Inlay::new(at, label_of(hint)?))
                })
                .collect();
            session
                .editor
                .set_inlays(self.document, HINT_INLAYS, inlays);
        });
    }

    /// Takes the lenses a server answered about the text at `version`: each
    /// with its title shown, and those without one resolved, the requests
    /// sent for them returned with the lens each is for.
    pub(super) fn take_lenses(
        &mut self,
        version: u64,
        answer: &Value,
        port: &Port,
    ) -> Vec<(u64, usize)> {
        if version != self.version() {
            return Vec::new();
        }
        let mut resolving = Vec::new();
        self.hints.lenses.clear();
        for lens in answer.as_array().into_iter().flatten() {
            let Some(line) = lens.pointer("/range/start/line").and_then(Value::as_u64) else {
                continue;
            };
            let at = self.hints.lenses.len();
            let title = title_of(lens);
            if title.is_none() {
                if let Some(id) = port.request("codeLens/resolve", lens.clone()) {
                    resolving.push((id, at));
                }
            }
            self.hints.lenses.push((line as usize, title));
        }
        self.lay_lenses();
        resolving
    }

    /// Takes the title of lens `at`, resolved, for the text at `version`.
    pub(super) fn take_resolved_lens(&mut self, version: u64, at: usize, answer: &Value) {
        if version != self.version() {
            return;
        }
        if let Some(lens) = self.hints.lenses.get_mut(at) {
            lens.1 = title_of(answer);
        }
        self.lay_lenses();
    }

    /// Lays the lenses with titles in, each after its line's last
    /// character, those of one line joined.
    fn lay_lenses(&mut self) {
        let mut by_line: Vec<(usize, Vec<&str>)> = Vec::new();
        for (line, title) in &self.hints.lenses {
            let Some(title) = title else {
                continue;
            };
            match by_line.iter_mut().find(|(known, _)| known == line) {
                Some((_, titles)) => titles.push(title),
                None => by_line.push((*line, vec![title])),
            }
        }
        session::with(|session| {
            let Some(text) = session.editor.document(self.document) else {
                return;
            };
            let inlays = by_line
                .into_iter()
                .filter_map(|(line, titles)| {
                    let start = text.position_to_char(viento::Position { column: 0, line })?;
                    let end = start + text.line_text(line)?.chars().count();
                    Some(Inlay::new(end, format!("{LENS_GAP}{}", titles.join(" | "))))
                })
                .collect();
            session
                .editor
                .set_inlays(self.document, LENS_INLAYS, inlays);
        });
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The char offset in `text` a server's position names.
fn offset_of(text: &viento::Document, position: &Value) -> Option<usize> {
    let line = position.get("line")?.as_u64()? as usize;
    let column = position.get("character")?.as_u64()? as usize;
    text.utf16_position_to_char(Utf16Position { column, line })
}

/// What a hint says: its label, a string or its parts joined, with the
/// room it asks for on either side.
fn label_of(hint: &Value) -> Option<String> {
    let label = match hint.get("label")? {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("value").and_then(Value::as_str))
            .collect(),
        _ => return None,
    };
    let pad = |side: &str| match hint.get(side).and_then(Value::as_bool) {
        Some(true) => " ",
        _ => "",
    };
    Some(format!(
        "{}{label}{}",
        pad("paddingLeft"),
        pad("paddingRight")
    ))
}

/// A lens's title, once its command is known.
fn title_of(lens: &Value) -> Option<String> {
    lens.pointer("/command/title")
        .and_then(Value::as_str)
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use winter_model::editor::Page;

    /// The id of the last request sent with `method`.
    fn asked(sent: &std::sync::mpsc::Receiver<Value>, method: &str) -> u64 {
        sent.try_iter()
            .filter(|message| message["method"] == method)
            .filter_map(|message| message["id"].as_u64())
            .last()
            .expect("the request went out")
    }

    #[test]
    fn test_hints_are_asked_once_typing_pauses_and_laid_in_and_a_lens_is_resolved() {
        let dir = std::env::temp_dir().join(format!("winter-hints-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let path = dir.join("main.rs");
        std::fs::write(&path, "let x = 1;\n").expect("written");
        let every = json!({"inlayHintProvider": true, "codeLensProvider": {}});
        let (port, sent) = Port::fake("rust", &dir, every);
        let server = port.id();
        let mut editor = TextEditor::new(path, None).expect("opens");
        editor.connect_lsp(1, &[port]);
        let now = Instant::now();
        editor.on_tick(now);
        assert!(sent
            .try_iter()
            .all(|message| message["method"] != "textDocument/inlayHint"));
        editor.on_tick(now + PAUSE);
        let messages: Vec<Value> = sent.try_iter().collect();
        let id_of = |method: &str| {
            messages
                .iter()
                .find(|message| message["method"] == method)
                .and_then(|message| message["id"].as_u64())
                .expect("asked")
        };
        let (hints, lenses_id) = (
            id_of("textDocument/inlayHint"),
            id_of("textDocument/codeLens"),
        );
        let at = json!({"line": 0, "character": 5});
        let answer = json!([{"position": at, "label": ": i32"}]);
        editor.on_lsp_answer(server, hints, &Ok(answer));
        let document = editor.document;
        session::with(|session| {
            let laid: Vec<_> = session.editor.inlays(document, HINT_INLAYS).to_vec();
            assert_eq!(laid, [Inlay::new(5, ": i32")]);
        });
        let lenses = json!([{"range": {"start": {"line": 0, "character": 0}}}]);
        editor.on_lsp_answer(server, lenses_id, &Ok(lenses));
        let resolve = asked(&sent, "codeLens/resolve");
        let resolved = json!({"command": {"title": "2 references"}});
        editor.on_lsp_answer(server, resolve, &Ok(resolved));
        session::with(|session| {
            let laid: Vec<_> = session.editor.inlays(document, LENS_INLAYS).to_vec();
            assert_eq!(laid, [Inlay::new(10, "  2 references")]);
        });
    }

    #[test]
    fn test_a_label_s_parts_are_joined_and_its_padding_kept() {
        let hint = json!({"label": [{"value": ": "}, {"value": "u32"}], "paddingLeft": true});
        assert_eq!(label_of(&hint).as_deref(), Some(" : u32"));
        assert_eq!(label_of(&json!({"label": "x:"})).as_deref(), Some("x:"));
        assert_eq!(
            title_of(&json!({"command": {"title": "3 references"}})).as_deref(),
            Some("3 references")
        );
        assert_eq!(title_of(&json!({"range": {}})), None, "unresolved");
    }
}
