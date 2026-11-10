//! Inlay hints: what a language server says of the code that the text does
//! not, drawn dim in the line where it applies, as a binding's type or a
//! call's parameter names. The whole file is asked for once typing pauses,
//! and what came back is drawn while it still fits the text: the line being
//! typed on loses its own until the next answer, and every line its own
//! once lines come or go.
//!
//! A hint takes cells after the character before it, so the cursor, a
//! click, a selection, and a wrapped line all count it where the painting
//! does (see `conceal::char_widths`).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use winter_lsp::protocol::{self, Position};
use winter_model::editor::{EditorOutcome, Notice};

use super::{lens, TextEditor};

// ========================================================================
// Constants
// ========================================================================

/// How long typing pauses before the hints are asked for again.
const HINT_PAUSE: Duration = Duration::from_millis(300);

/// What a server says it can do, for inlay hints.
const PROVIDER: &str = "inlayHintProvider";

/// No hints, for a line whose own no longer fit it.
static EMPTY: Hints = Hints {
    asked: None,
    due: None,
    held: BTreeMap::new(),
    hidden: false,
    answered: None,
};

/// How the hints' toggle is said.
const HINTS_ON: &str = "inlay hints on";
const HINTS_OFF: &str = "inlay hints off";

// ========================================================================
// Data Structures
// ========================================================================

/// One document's inlay hints, and when to ask for them again.
#[derive(Debug, Default)]
pub(super) struct Hints {
    /// The revision of the text last asked about, so it is asked once.
    asked: Option<usize>,
    /// When to ask, once typing has paused.
    due: Option<Instant>,
    /// The hints last answered, by line: each the character it stands
    /// before, and what it says.
    held: BTreeMap<usize, Vec<(usize, String)>>,
    /// Whether they are hidden, and not asked for.
    hidden: bool,
    /// The text's line count and revision the hints held were answered for.
    answered: Option<(usize, usize)>,
}

// ========================================================================
// TextEditor: inlay hints
// ========================================================================

impl TextEditor {
    /// When the hints are next to be asked for.
    pub(super) fn hints_next_tick(&self) -> Option<Instant> {
        self.doc().hints.due
    }

    /// Ask for the hints where the text has changed since they were last
    /// asked about and typing has paused, from a server that has them.
    pub(super) fn hints_tick(&mut self, now: Instant) {
        let revision = self.buffer().revision();
        let hints = &self.doc().hints;
        if hints.hidden || hints.asked == Some(revision) {
            return;
        }
        let Some(port) = self
            .serving_port()
            .filter(|port| port.provides(PROVIDER) || port.provides(lens::PROVIDER))
            .cloned()
        else {
            return;
        };
        match self.doc().hints.due {
            None => {
                self.doc_mut().hints.due = Some(now + HINT_PAUSE);
                return;
            }
            Some(due) if due > now => return,
            Some(_) => {}
        }
        let lines = self.buffer().lines().len();
        let params = json!({
            "textDocument": {"uri": protocol::uri_of(&self.doc().path)},
            "range": {
                "start": {"line": 0, "character": 0},
                "end": {"line": lines, "character": 0},
            },
        });
        let hints = &mut self.doc_mut().hints;
        hints.due = None;
        hints.asked = Some(revision);
        if port.provides(lens::PROVIDER) {
            self.ask_lenses(&port, revision);
        }
        if !port.provides(PROVIDER) {
            return;
        }
        if let Some(id) = port.request("textDocument/inlayHint", params) {
            self.ask_hints(port.id(), id, revision);
        }
    }

    /// Take the hints a server answered about the text at `revision`.
    pub(super) fn take_hints(&mut self, revision: usize, value: &Value) -> EditorOutcome {
        let lines = self.buffer().lines();
        let mut held: BTreeMap<usize, Vec<(usize, String)>> = BTreeMap::new();
        for hint in value.as_array().into_iter().flatten() {
            let (Some(at), Some(label)) =
                (hint.get("position").and_then(position_of), label_of(hint))
            else {
                continue;
            };
            if at.line as usize >= lines.len() {
                continue;
            }
            let (row, col) = protocol::place_of(lines, at);
            held.entry(row).or_default().push((col, label));
        }
        let count = lines.len();
        let hints = &mut self.doc_mut().hints;
        hints.held = held;
        hints.answered = Some((count, revision));
        EditorOutcome::Consumed
    }

    /// The hints drawn in line `index`, each after the character it says
    /// it follows and what it says. None where they no longer fit the text.
    pub(super) fn hints_after(&self, index: usize) -> Vec<(usize, String)> {
        let hints = &self.doc().hints;
        if hints.hidden {
            return Vec::new();
        }
        let fits = hints.answered.is_some_and(|(lines, revision)| {
            revision == self.buffer().revision()
                || (lines == self.buffer().lines().len() && index != self.buffer().row())
        });
        let hints = match fits {
            true => hints,
            false => &EMPTY,
        };
        let length = self.buffer().line_len_of(index);
        let mut after: Vec<(usize, String)> = hints
            .held
            .get(&index)
            .into_iter()
            .flatten()
            // A hint before the line's first character is left out: it
            // would take the cells the cursor stands in to type there.
            .filter(|(col, _)| *col > 0)
            .map(|(col, label)| ((*col).min(length) - 1, label.clone()))
            .collect();
        // The line's code lenses, after its last character.
        if let (Some(label), Some(last)) = (self.lens_label(index), length.checked_sub(1)) {
            after.push((last, label));
        }
        after
    }

    /// Show the hints, or hide them.
    pub(super) fn toggle_hints(&mut self) -> EditorOutcome {
        let hints = &mut self.doc_mut().hints;
        hints.hidden = !hints.hidden;
        hints.asked = None;
        let said = if hints.hidden { HINTS_OFF } else { HINTS_ON };
        self.modal.report(Notice::info(said).unlogged());
        EditorOutcome::Consumed
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The cells `hints` take after character `col`.
pub(super) fn width_after(hints: &[(usize, String)], col: usize) -> usize {
    hints
        .iter()
        .filter(|(after, _)| *after == col)
        .map(|(_, label)| label.chars().count())
        .sum()
}

/// A position from the wire.
fn position_of(value: &Value) -> Option<Position> {
    Some(Position {
        character: u32::try_from(value.get("character")?.as_u64()?).ok()?,
        line: u32::try_from(value.get("line")?.as_u64()?).ok()?,
    })
}

/// What a hint says: its label, text or parts, with the space the server
/// asks for on either side.
fn label_of(hint: &Value) -> Option<String> {
    let label = hint.get("label")?;
    let text = match label {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("value").and_then(Value::as_str))
            .collect(),
        _ => return None,
    };
    let pad = |key: &str| hint.get(key).and_then(Value::as_bool).unwrap_or(false);
    let before = if pad("paddingLeft") { " " } else { "" };
    let after = if pad("paddingRight") { " " } else { "" };
    let text = text.replace(['\n', '\t'], " ");
    (!text.is_empty()).then(|| format!("{before}{text}{after}"))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_label_is_its_parts_with_the_padding_asked_for() {
        let typed = json!({"label": ": Vec<u8>", "paddingLeft": false});
        assert_eq!(label_of(&typed).as_deref(), Some(": Vec<u8>"));
        let named = json!({
            "label": [{"value": "count"}, {"value": ":"}],
            "paddingRight": true,
        });
        assert_eq!(label_of(&named).as_deref(), Some("count: "));
        assert_eq!(label_of(&json!({"label": ""})), None);
    }
}
