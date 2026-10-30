//! The text editor's tests: language servers.

use super::*;
use winter_lsp::protocol::{self, Diagnostic, Position, Range, Severity};
use winter_lsp::{Event, Incoming, Port};
use winter_lsp::diagnostics::{self, Backend};
use crate::model::editor::{
    LSP_DEFINITION, NAVIGATE_BACK, PROBLEM_FIRST_WRITING, PROBLEM_NEXT_ERROR, PROBLEM_NEXT_IN_FILE,
};
use serde_json::{json, Value};
use std::sync::mpsc::Receiver;

/// Everything sent to the server so far, by method.
fn sent_methods(sent: &Receiver<Value>) -> Vec<Value> {
    sent.try_iter().collect()
}

/// The server's answer to request `id`.
fn answer(id: u64, result: Value) -> Incoming {
    Incoming {
        event: Event::Answer { id, result: Ok(result) },
        server: 1,
    }
}

/// The id of the last request sent for `method`.
fn request_id(sent: &[Value], method: &str) -> u64 {
    sent.iter()
        .rev()
        .find(|message| message["method"] == method)
        .and_then(|message| message["id"].as_u64())
        .unwrap_or_else(|| panic!("no {method} request in {sent:?}"))
}

fn served(dir: &TempDir, capabilities: Value) -> (Port, Receiver<Value>) {
    Port::fake("rust", &dir.0, capabilities)
}

#[test]
fn test_a_served_file_is_opened_with_the_server_and_its_edits_handed_over() {
    let dir = TempDir::new("lsp-sync");
    let path = dir.file("main.rs", "fn main() {}\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    let (port, sent) = served(&dir, json!({}));
    editor.connect_lsp(1, std::slice::from_ref(&port));
    let opened = sent_methods(&sent);
    assert_eq!(opened[0]["method"], "textDocument/didOpen");
    assert_eq!(opened[0]["params"]["textDocument"]["languageId"], "rust");
    assert_eq!(opened[0]["params"]["textDocument"]["text"], "fn main() {}\n");
    type_keys(&mut editor, "ix<esc>");
    assert!(editor.next_tick().is_some(), "the edit is due to be handed over");
    editor.on_tick(Instant::now());
    let changed = sent_methods(&sent);
    assert_eq!(changed[0]["method"], "textDocument/didChange");
    assert_eq!(changed[0]["params"]["textDocument"]["version"], 2);
    assert_eq!(changed[0]["params"]["contentChanges"][0]["text"], "xfn main() {}\n");
    editor.connect_lsp(1, std::slice::from_ref(&port));
    editor.on_tick(Instant::now());
    assert!(sent_methods(&sent).is_empty(), "nothing new to hand over");
}

#[test]
fn test_a_file_going_to_root_is_closed_with_its_server_under_the_path_it_opened_as() {
    // Its path became sudo's first, and the close went out later for a
    // URI the server had never opened, leaving its own open for good.
    let dir = TempDir::new("lsp-as-root");
    let path = dir.file("main.rs", "fn main() {}\n");
    let mut editor = TextEditor::new(path.clone(), None).expect("opens");
    let (port, sent) = served(&dir, json!({}));
    editor.connect_lsp(1, std::slice::from_ref(&port));
    sent_methods(&sent);
    editor.tell_doc_closed(0);
    editor.doc_mut().path = crate::remote::Remote::sudo(&path).to_path();
    let closed = sent_methods(&sent);
    assert_eq!(closed[0]["method"], "textDocument/didClose");
    assert_eq!(closed[0]["params"]["textDocument"]["uri"], winter_lsp::protocol::uri_of(&path));
    editor.tell_closed();
    editor.connect_lsp(1, std::slice::from_ref(&port));
    assert!(sent_methods(&sent).is_empty(), "nothing more for a file through sudo");
}

#[test]
fn test_the_fixes_list_takes_spelling_s_and_the_server_s_in_one_popup() {
    let dir = TempDir::new("lsp-fixes");
    let path = dir.file("main.rs", "// we recieve it\nfn main() {}\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    let (port, sent) = served(&dir, json!({"codeActionProvider": true}));
    editor.connect_lsp(1, &[port]);
    // A program's comments are checked once asked for.
    editor.on_key(&Key {
        alt: true,
        shift: true,
        ..key(KeyCode::Char('i'))
    });
    editor.content(10, 60, false);
    type_keys(&mut editor, "6l");
    let asked = editor.on_key(&Key {
        alt: true,
        ..key(KeyCode::Char('\\'))
    });
    assert_eq!(asked, EditorOutcome::Consumed, "the server is waited for");
    let id = request_id(&sent_methods(&sent), "textDocument/codeAction");
    let actions = json!([{"title": "Allow the lint", "command": {"command": "allow"}}]);
    let EditorOutcome::Pick(pick) = editor.on_lsp(&answer(id, actions)) else {
        panic!("one list of every source's fixes");
    };
    assert_eq!(pick.items[0], "Change to \"receive\"", "{:?}", pick.items);
    assert_eq!(pick.placement, crate::model::editor::PickPlacement::Cursor, "the fixes float at the cursor");
    let last = pick.items.last().map(String::as_str).unwrap_or_default();
    assert_eq!(last, "Allow the lint", "the server's after");
    editor.on_prompt(PromptReply {
        answer: pick.items.last().cloned(),
        tag: pick.tag,
    });
    let ran = request_id(&sent_methods(&sent), "workspace/executeCommand");
    assert!(ran > id, "the server's fix is made by the server");
}

#[test]
fn test_a_server_hears_of_a_file_once_however_many_tabs_show_it() {
    let dir = TempDir::new("lsp-one-buffer");
    let path = dir.file("main.rs", "fn main() {}\n");
    let (port, sent) = served(&dir, json!({}));
    let mut first = TextEditor::new(path.clone(), None).expect("opens");
    first.connect_lsp(1, std::slice::from_ref(&port));
    let mut second = TextEditor::new(path, None).expect("joins");
    second.connect_lsp(1, std::slice::from_ref(&port));
    let methods = |sent: &Receiver<Value>| -> Vec<String> {
        sent_methods(sent).iter().map(|message| message["method"].as_str().unwrap_or_default().to_string()).collect()
    };
    assert_eq!(methods(&sent), ["textDocument/didOpen"], "opened once");
    first.on_closed();
    assert!(methods(&sent).is_empty(), "still open in the second tab");
    second.on_closed();
    assert_eq!(methods(&sent), ["textDocument/didClose"], "closed with the last tab");
}

#[test]
fn test_a_definition_in_another_file_is_jumped_to_and_back_returns() {
    let dir = TempDir::new("lsp-definition");
    let main = dir.file("main.rs", "fn main() {\n    helper();\n}\n");
    let other = dir.file("other.rs", "pub fn helper() {}\n");
    let mut editor = TextEditor::new(main.clone(), None).expect("opens");
    let (port, sent) = served(&dir, json!({"definitionProvider": true}));
    editor.connect_lsp(1, &[port]);
    type_keys(&mut editor, "jw");
    editor.run_command(LSP_DEFINITION);
    let id = request_id(&sent_methods(&sent), "textDocument/definition");
    let location = json!({"uri": protocol::uri_of(&other),
        "range": {"start": {"line": 0, "character": 7}, "end": {"line": 0, "character": 13}}});
    editor.on_lsp(&answer(id, location));
    assert_eq!(editor.doc().path, main, "this editor keeps its own file");
    assert_eq!(
        editor.take_requests(),
        [EditorRequest::Open(OpenTarget {
            column: Some(8),
            line: Some(1),
            path: other.clone(),
        })],
        "and asks the window to show the other at the definition"
    );
    // Back, from the editor the window showed the definition in, is a
    // jump into the first file: the history is the window's.
    let mut shown = TextEditor::new(other, None).expect("opens");
    shown.run_command(NAVIGATE_BACK);
    assert_eq!(
        shown.take_requests(),
        [EditorRequest::Open(OpenTarget {
            column: Some(5),
            line: Some(2),
            path: main,
        })]
    );
}

#[test]
fn test_the_problem_the_pointer_rests_on_is_outlined_while_its_card_is_open() {
    let dir = TempDir::new("hover-problem");
    let path = dir.file("main.rs", "fn main() {\n    helper();\n}\n");
    let mut editor = TextEditor::new(path.clone(), None).expect("opens");
    let problem = Diagnostic {
        code: None,
        message: "cannot find function `helper`".to_string(),
        range: Range {
            end: Position { character: 10, line: 1 },
            start: Position { character: 4, line: 1 },
        },
        severity: Severity::Error,
        source: Some("rustc".to_string()),
    };
    diagnostics::publish(Backend::Lsp(1), &path, vec![problem]);
    editor.connect_lsp(1, &[]);
    editor.content(10, 60, false);
    // The pane cell the `l` of `helper` is painted on.
    let col = (0..60)
        .find(|&col| editor.char_at_cell(Point { col, drag: false, row: 1 }) == Some((1, 6)))
        .expect("the word is painted");
    let lit_text = |editor: &mut TextEditor| -> String {
        let frame = editor.content(10, 60, false);
        frame.rows[1].iter().filter(|span| span.lit).map(|span| span.text.as_str()).collect()
    };
    assert_eq!(lit_text(&mut editor), "", "nothing outlined before the pointer rests");
    editor.on_hover(Some(Point { col, drag: false, row: 1 }));
    editor.on_tick(Instant::now() + Duration::from_secs(1));
    assert!(editor.popups().card.is_some(), "the card says what the problem is");
    assert_eq!(lit_text(&mut editor), "helper", "the problem it describes is outlined");
    editor.on_hover(None);
    assert_eq!(lit_text(&mut editor), "", "and let go once the pointer leaves");
}

#[test]
fn test_problems_are_marked_under_the_text_and_stepped_through() {
    let dir = TempDir::new("lsp-problems");
    let path = dir.file("main.rs", "fn main() {\n    helper();\n}\n");
    let mut editor = TextEditor::new(path.clone(), None).expect("opens");
    let (port, _sent) = served(&dir, json!({}));
    editor.connect_lsp(1, std::slice::from_ref(&port));
    let problem = Diagnostic {
        code: Some("E0425".to_string()),
        message: "cannot find function `helper`".to_string(),
        range: Range {
            end: Position { character: 10, line: 1 },
            start: Position { character: 4, line: 1 },
        },
        severity: Severity::Error,
        source: Some("rustc".to_string()),
    };
    diagnostics::publish(Backend::Lsp(1), &path, vec![problem]);
    let said = Incoming {
        event: Event::Diagnostics(path),
        server: 1,
    };
    assert_eq!(editor.on_lsp(&said), EditorOutcome::Consumed);
    let marks = editor.squiggles(1, 13);
    assert_eq!((marks[3], marks[4], marks[9], marks[10]), (None, Some(DiagnosticLevel::Error), Some(DiagnosticLevel::Error), None));
    assert!(editor.status().segments.contains(&"E1 W0".to_string()));
    // The problem is marked on its text alone: its line's number is drawn
    // like the others, and only the cursor's line number stands apart.
    let numbers: Vec<Style> = editor.content(10, 40, false).rows.iter().map(|row| row[0].style).collect();
    assert_eq!(numbers[..3], [Style::LineNumberActive, Style::Dim, Style::Dim]);
    editor.run_command(PROBLEM_NEXT_IN_FILE);
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (1, 4));
    assert!(!editor.is_flashing(), "the next problem in the file does not flash the line");
    let card = editor.popups().card.expect("the problem is said");
    assert!(card.text.contains("cannot find function"), "{}", card.text);
    assert_eq!(card.footer.as_deref(), Some("error rustc(E0425)"), "the kind at its foot");
    editor.on_key(&key(KeyCode::Char('l')));
    assert_eq!(editor.popups().card, None, "a key closes the card");
}

#[test]
fn test_problems_from_every_backend_are_stepped_through_and_filtered() {
    // A server's error on line 2 and a word spelled wrong on line 0:
    // every problem is stepped through in order, the errors alone skip
    // the word, and the writing issues alone skip the error.
    let dir = TempDir::new("problems-backends");
    let path = dir.file("notes.md", "a wrod here

fine text
");
    let mut editor = TextEditor::new(path.clone(), None).expect("opens");
    let error = Diagnostic {
        code: None,
        message: "broken link".to_string(),
        range: Range {
            end: Position { character: 4, line: 2 },
            start: Position { character: 0, line: 2 },
        },
        severity: Severity::Error,
        source: Some("marksman".to_string()),
    };
    diagnostics::publish(Backend::Lsp(1), &path, vec![error]);
    editor.run_command(PROBLEM_NEXT_IN_FILE);
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (2, 0), "the error, after the word under the cursor");
    editor.run_command(PROBLEM_NEXT_IN_FILE);
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (0, 2), "round to the word");
    assert!(editor.popups().card.expect("said").text.contains("wrod"));
    assert_eq!(editor.squiggles(0, 11)[2], Some(DiagnosticLevel::Info), "the word is underlined");
    editor.run_command(PROBLEM_NEXT_ERROR);
    assert_eq!(editor.buffer().row(), 2, "the errors alone");
    editor.run_command(PROBLEM_FIRST_WRITING);
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (0, 2), "the writing issues alone");
}

#[test]
fn test_a_server_s_completion_opens_the_popup_and_brings_its_import() {
    let dir = TempDir::new("lsp-completion");
    let path = dir.file("main.rs", "fn main() {\n    \n}\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    let (port, sent) = served(&dir, json!({"completionProvider": {}}));
    editor.connect_lsp(1, &[port]);
    type_keys(&mut editor, "jA");
    press(&mut editor, "Ha");
    editor.on_key(&Key {
        ctrl: true,
        ..key(KeyCode::Space)
    });
    let id = request_id(&sent_methods(&sent), "textDocument/completion");
    let items = json!([{"label": "HashMap", "kind": 22, "detail": "std::collections",
        "additionalTextEdits": [{"newText": "use std::collections::HashMap;\n",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}}]}]);
    editor.on_lsp(&answer(id, items));
    let popup = editor.popups().completion.expect("the server's answer opens the popup");
    assert_eq!((popup.items[0].label.as_str(), popup.items[0].kind, popup.items[0].source), ("HashMap", 't', "lsp"));
    editor.on_key(&key(KeyCode::Tab));
    let lines = editor.buffer().lines();
    assert_eq!(lines[0], "use std::collections::HashMap;");
    assert_eq!(lines[2], "    HashMap");
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (2, 11), "still after the word typed");
}
