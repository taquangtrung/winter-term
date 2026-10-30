//! The text editor's tests: the context menu and the clipboard.

use super::*;

#[test]
fn test_the_clipboard_register_leaves_the_editor() {
    let tmp = TempDir::new("clipboard");
    let mut editor = tmp.editor("one\ntwo\n");
    editor.on_key(&key(KeyCode::Char('"')));
    editor.on_key(&key(KeyCode::Char('+')));
    press(&mut editor, "y");
    let outcome = editor.on_key(&key(KeyCode::Char('y')));
    assert_eq!(outcome, EditorOutcome::Yank("one\n".to_string()), "whole lines end in a newline");
}

#[test]
fn test_the_clipboard_register_asks_the_host_for_what_to_put_back() {
    let tmp = TempDir::new("clipboard-put");
    let mut editor = tmp.editor("one\n");
    editor.on_key(&key(KeyCode::Char('"')));
    editor.on_key(&key(KeyCode::Char('+')));
    let outcome = editor.on_key(&key(KeyCode::Char('p')));
    assert_eq!(
        outcome,
        EditorOutcome::Paste,
        "the page cannot read it itself"
    );
    editor.on_paste("two\n".to_string());
    assert_eq!(
        editor.buffer().lines(),
        ["one", "two"],
        "whole lines, as copied"
    );
}

#[test]
fn test_the_menu_copies_a_selection_to_the_clipboard_and_keeps_it_on_right_click() {
    let tmp = TempDir::new("menu-copy");
    let mut editor = tmp.editor("alpha beta\n");
    paint_frame(&mut editor, 10);
    press(&mut editor, "ve");
    editor.on_context_mouse(Point {
        col: 30,
        drag: false,
        row: 0,
    });
    assert!(
        editor.modal.is_selecting(),
        "a right-click keeps the selection"
    );
    assert_eq!(menu_labels(&editor), ["Copy", "Cut", "Toggle Comment"]);
    assert_eq!(
        choose(&mut editor, "Copy"),
        EditorOutcome::Yank("alpha".to_string())
    );
}

#[test]
fn test_the_menu_pastes_from_the_clipboard_in_normal_mode() {
    let tmp = TempDir::new("menu-paste");
    let mut editor = tmp.editor("alpha\n");
    let labels = menu_labels(&editor);
    assert_eq!(labels.first().map(String::as_str), Some("Paste"));
    assert!(labels.contains(&"Undo".to_string()) && labels.contains(&"Save".to_string()));
    assert_eq!(choose(&mut editor, "Paste"), EditorOutcome::Paste);
}

#[test]
fn test_every_normal_mode_menu_entry_is_a_key_the_editor_answers() {
    let tmp = TempDir::new("menu-keys");
    for label in menu_labels(&tmp.editor("alpha\n")) {
        let mut editor = tmp.editor("alpha\n");
        assert_ne!(
            choose(&mut editor, &label),
            EditorOutcome::Ignored,
            "{label}"
        );
    }
}
