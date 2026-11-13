//! Completion from the language server serving a document, as one of
//! Cherry's completion providers. Cherry asks while a key is being handled,
//! when the server cannot be: the provider only notes the ask, and the tab
//! sends it once the key is done (the text handed over first), answering
//! Cherry's ticket when the server answers.

use std::sync::{Arc, Mutex, PoisonError};

use cherry::{
    CompletionAnswer, CompletionItem, CompletionKind, CompletionProvider, CompletionRequest,
    CompletionTicket, DocumentId, Utf16Position,
};
use winter_lsp::protocol::{self, CompletionEntry};
use winter_lsp::{Port, ServerId};

use super::session::Session;

// ========================================================================
// Data Structures
// ========================================================================

/// The asks a document's provider noted and its tab has yet to send: the
/// ticket each is answered under, and the char offset it is about.
pub(crate) type Asks = Arc<Mutex<Vec<(CompletionTicket, usize)>>>;

/// A document's server as a completion provider.
#[derive(Debug)]
struct ServerCompletion {
    /// Where the asks go until the tab sends them.
    asks: Asks,
    /// The server's name, shown by its matches.
    name: String,
    /// What typed asks the server at once.
    triggers: Vec<char>,
}

// ========================================================================
// ServerCompletion Implementations
// ========================================================================

impl CompletionProvider for ServerCompletion {
    fn complete(&self, request: &CompletionRequest<'_>) -> CompletionAnswer {
        self.asks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((request.ticket, request.offset));
        CompletionAnswer::Later
    }

    fn source(&self) -> Option<&str> {
        Some(&self.name)
    }

    fn trigger_characters(&self) -> &[char] {
        &self.triggers
    }
}

// ========================================================================
// Session: completion
// ========================================================================

impl Session {
    /// Makes `port` the completion provider of `document`, once, where it
    /// completes.
    pub(crate) fn complete_with(&mut self, document: DocumentId, port: &Port) {
        let Some(file) = self.files.get_mut(&document) else {
            return;
        };
        if file.completing == Some(port.id()) || !port.provides("completionProvider") {
            return;
        }
        file.completing = Some(port.id());
        let provider = ServerCompletion {
            asks: Arc::clone(&file.asks),
            name: port.name().to_owned(),
            triggers: port.trigger_characters(),
        };
        self.editor
            .set_completion_providers(document, vec![Box::new(provider)]);
    }

    /// Sends the asks `document`'s provider noted to `port`, keeping the
    /// ticket each is answered under.
    pub(crate) fn send_asks(&mut self, document: DocumentId, port: &Port) {
        let Some(file) = self.files.get(&document) else {
            return;
        };
        let asks: Vec<_> = file
            .asks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect();
        let path = file.path.clone();
        let Some(text) = self.editor.document(document) else {
            return;
        };
        for (ticket, offset) in asks {
            let Some(Utf16Position { column, line }) = text.char_to_utf16_position(offset) else {
                continue;
            };
            let at = protocol::Position {
                character: u32::try_from(column).unwrap_or(u32::MAX),
                line: u32::try_from(line).unwrap_or(u32::MAX),
            };
            let params = protocol::at_params(&path, at);
            if let Some(id) = port.request("textDocument/completion", params) {
                self.completions.insert((port.id(), id), ticket);
            }
        }
    }

    /// Hands the matches the server answered request `id` with to the
    /// ticket it was sent under. Whether the request was a completion's.
    pub(crate) fn answer_completion(
        &mut self,
        server: ServerId,
        id: u64,
        answer: &serde_json::Value,
    ) -> bool {
        let Some(ticket) = self.completions.remove(&(server, id)) else {
            return false;
        };
        let items = protocol::completions(answer)
            .entries
            .iter()
            .map(item_of)
            .collect();
        self.editor.answer_completion(ticket, items);
        true
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What a server offered, as Cherry's menu shows it.
fn item_of(entry: &CompletionEntry) -> CompletionItem {
    let item = CompletionItem::new(&entry.insert);
    let item = CompletionItem {
        detail: entry.detail.clone(),
        label: entry.label.clone(),
        ..item
    };
    let item = match entry.kind.and_then(kind_of) {
        Some(kind) => item.with_kind(kind),
        None => item,
    };
    match entry.snippet {
        true => item.with_snippet(),
        false => item,
    }
}

/// The kind of thing a server's kind number names, as Cherry sorts them.
fn kind_of(kind: u64) -> Option<CompletionKind> {
    Some(match kind {
        1 => CompletionKind::Text,
        2 => CompletionKind::Method,
        3 | 4 => CompletionKind::Function,
        5 | 10 => CompletionKind::Field,
        6 | 12 => CompletionKind::Variable,
        7 | 8 | 22 => CompletionKind::Class,
        9 => CompletionKind::Module,
        13 | 20 => CompletionKind::Enum,
        14 => CompletionKind::Keyword,
        15 => CompletionKind::Snippet,
        17 | 19 => CompletionKind::File,
        21 => CompletionKind::Constant,
        25 => CompletionKind::Type,
        _ => return None,
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use serde_json::json;
    use winter_model::editor::Page;
    use winter_model::input::{Key, KeyCode};

    use crate::{session, TextEditor};

    /// `code` pressed, with Ctrl when `ctrl` is set.
    fn press(code: KeyCode, ctrl: bool) -> Key {
        Key {
            alt: false,
            code,
            ctrl,
            shift: false,
            sup: false,
        }
    }

    #[test]
    fn test_an_ask_goes_to_the_server_after_the_key_and_its_answer_fills_the_menu() {
        let dir = std::env::temp_dir().join(format!("winter-completing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        let path = dir.join("main.rs");
        std::fs::write(&path, "pri\n").expect("written");
        let capabilities = json!({"completionProvider": {"triggerCharacters": ["."]}});
        let (port, sent) = winter_lsp::Port::fake("rust", &dir, capabilities);
        let server = port.id();
        let mut editor = TextEditor::new(path, None).expect("opens");
        editor.connect_lsp(1, &[port]);
        for key in [
            press(KeyCode::Char('A'), false),
            press(KeyCode::Char('x'), true),
            press(KeyCode::Char('o'), true),
        ] {
            editor.on_key(&key);
        }
        let id = sent
            .try_iter()
            .filter(|message| message["method"] == "textDocument/completion")
            .find_map(|message| message["id"].as_u64())
            .expect("asked after the key");
        let answer = json!([{"label": "println!", "insertText": "println!", "kind": 3}]);
        let view = editor.view_id();
        session::with(|session| {
            assert!(session.answer_completion(server, id, &answer));
            let menu = session.editor.view(view).and_then(cherry::View::completion);
            let labels: Vec<_> = menu
                .map(|menu| menu.items().iter().map(|item| item.label.clone()).collect())
                .unwrap_or_default();
            assert_eq!(labels, ["println!"]);
        });
    }
}
