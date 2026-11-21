//! Document symbols: what a language server says a file defines, its
//! functions, types, and modules, each as deep as what holds it. They are
//! asked for once typing pauses, of the whole file, and the outline lists
//! them first while they are of the text as it is.

use std::time::{Duration, Instant};

use cherry::markup::OutlineEntry;
use serde_json::{Value, json};
use winter_lsp::Port;
use winter_lsp::protocol::{self, Symbol};

use super::{TextEditor, session};

// ========================================================================
// Constants
// ========================================================================

/// How long typing pauses before the symbols are asked for again.
const PAUSE: Duration = Duration::from_millis(300);

/// What a server says it can do, for document symbols.
const PROVIDER: &str = "documentSymbolProvider";

// ========================================================================
// Data Structures
// ========================================================================

/// The request a tab sent for the symbols of the text at a version.
pub(super) struct SymbolAsk {
    /// The request's id.
    pub(super) id: u64,
    /// The server asked.
    pub(super) port: Port,
    /// The text's version asked about.
    pub(super) version: u64,
}

/// One tab's symbols: when to ask, and what came back.
#[derive(Debug, Default)]
pub(super) struct Symbols {
    /// The text's version last asked about, so it is asked once.
    asked: Option<u64>,
    /// When to ask, once typing has paused.
    due: Option<Instant>,
    /// The outline answered, and the text's version it is of.
    held: Option<(u64, Vec<OutlineEntry>)>,
}

// ========================================================================
// TextEditor: symbols
// ========================================================================

impl TextEditor {
    /// When the symbols are next to be asked for.
    pub(super) fn symbols_next_tick(&self) -> Option<Instant> {
        self.symbols.due
    }

    /// Asks for the symbols where the text changed since they were last
    /// asked about and typing has paused, of a server that has them.
    pub(super) fn symbols_tick(&mut self, now: Instant) -> Option<SymbolAsk> {
        let version = self.version();
        if self.symbols.asked == Some(version) {
            return None;
        }
        let port = self.port().filter(|port| port.provides(PROVIDER))?;
        match self.symbols.due {
            None => {
                self.symbols.due = Some(now + PAUSE);
                return None;
            }
            Some(due) if due > now => return None,
            Some(_) => {}
        }
        self.symbols.due = None;
        self.symbols.asked = Some(version);
        let params = json!({"textDocument": {"uri": protocol::uri_of(&self.path)}});
        let id = port.request("textDocument/documentSymbol", params)?;
        Some(SymbolAsk { id, port, version })
    }

    /// Takes the symbols a server answered about the text at `version`.
    pub(super) fn take_symbols(&mut self, version: u64, answer: &Value) {
        // Lines a server counted in text changed since would land wrong.
        if version != self.version() {
            return;
        }
        let lines = session::with(|session| session.lines(self.document));
        let outline = outline_of(&lines, &protocol::document_symbols(answer));
        self.symbols.held = Some((version, outline));
    }

    /// The outline the server gave for the text as it is, where it gave
    /// one with anything in it.
    pub(super) fn served_outline(&self) -> Option<Vec<OutlineEntry>> {
        let (version, outline) = self.symbols.held.as_ref()?;
        (*version == self.version() && !outline.is_empty()).then(|| outline.clone())
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The lines `symbols` name in `lines`, each as deep as its symbol sits and
/// called by its line's text, one entry to a line.
fn outline_of(lines: &[String], symbols: &[Symbol]) -> Vec<OutlineEntry> {
    let mut outline: Vec<OutlineEntry> = Vec::new();
    for symbol in symbols {
        let (line, _) = protocol::place_of(lines, symbol.at);
        let Some(text) = lines.get(line) else {
            continue;
        };
        if outline.last().is_some_and(|last| last.line == line) {
            continue;
        }
        outline.push(OutlineEntry {
            level: symbol.depth + 1,
            line,
            title: text.trim().to_string(),
        });
    }
    outline
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_lsp::protocol::Position;
    use winter_model::editor::Page;

    use super::*;

    #[test]
    fn test_the_outline_lists_a_server_s_symbols_of_the_text_as_it_is() {
        let dir = std::env::temp_dir().join(format!("winter-symbols-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let path = dir.join("main.rs");
        std::fs::write(&path, "struct S;\nimpl S {\n    fn new() {}\n}\n").expect("written");
        let (port, sent) = Port::fake("rust", &dir, json!({"documentSymbolProvider": true}));
        let server = port.id();
        let mut editor = TextEditor::new(path, None).expect("opens");
        editor.connect_lsp(1, &[port]);
        let labels = |editor: &mut TextEditor| -> Vec<String> {
            let targets = editor.outline_targets().expect("an outline");
            targets.into_iter().map(|target| target.label).collect()
        };
        let now = Instant::now();
        editor.on_tick(now);
        editor.on_tick(now + PAUSE);
        let id = sent
            .try_iter()
            .filter(|message| message["method"] == "textDocument/documentSymbol")
            .find_map(|message| message["id"].as_u64())
            .expect("asked once typing paused");
        let symbol = |line: u32, children: Value| {
            let at = json!({"line": line, "character": 0});
            json!({"name": "x", "kind": 23, "range": {"start": at, "end": at},
                "selectionRange": {"start": at, "end": at}, "children": children})
        };
        let answer = json!([symbol(1, json!([symbol(2, json!([]))]))]);
        editor.on_lsp_answer(server, id, &Ok(answer));
        assert_eq!(
            labels(&mut editor),
            ["    2  impl S {", "    3    fn new() {}"],
            "the server's, nested by its tree"
        );
        let view = editor.view_id();
        session::with(|session| session.editor.insert_lines(view, 0, "// x"));
        assert_eq!(
            labels(&mut editor)[0],
            "    2  struct S;",
            "an answer about the text before an edit is the grammar's outline again"
        );
    }

    #[test]
    fn test_symbols_are_one_entry_to_a_line_by_its_text_and_none_past_the_end() {
        let lines: Vec<String> = ["struct S {", "    a: u8, b: u8,", "}", "fn f() {}"]
            .map(str::to_string)
            .to_vec();
        let at = |line: u32, depth: usize| Symbol {
            at: Position { character: 4, line },
            depth,
        };
        let found: Vec<(usize, usize, String)> =
            outline_of(&lines, &[at(0, 0), at(1, 1), at(1, 1), at(3, 0), at(9, 0)])
                .into_iter()
                .map(|entry| (entry.level, entry.line, entry.title))
                .collect();
        assert_eq!(
            found,
            [
                (1, 0, "struct S {".to_string()),
                (2, 1, "a: u8, b: u8,".to_string()),
                (1, 3, "fn f() {}".to_string()),
            ]
        );
    }
}
