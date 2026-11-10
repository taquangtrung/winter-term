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
    assert_eq!(
        outcome,
        EditorOutcome::Yank("one\n".to_string()),
        "whole lines end in a newline"
    );
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

#[test]
fn test_file_reference_adds_lines_only_for_a_visual_selection() {
    let tmp = TempDir::new("file-ref");
    let mut editor = tmp.editor("line one\nline two\nline three\n");

    // Normal mode: the path alone, wherever the cursor rests.
    editor.buffer_mut().move_to(1, 0);
    let plain = editor.file_reference().expect("reference");
    assert!(!plain.contains(':'), "expected no line, got {plain}");

    // Visual mode on one line: that line.
    press(&mut editor, "V");
    let ref_line = editor.file_reference().expect("visual reference");
    assert!(ref_line.ends_with(":2"), "expected :2, got {ref_line}");
    editor.on_key(&key(KeyCode::Escape));

    // Visual mode over several lines: the range.
    press(&mut editor, "Vj");
    let ref_visual = editor.file_reference().expect("visual reference");
    assert!(
        ref_visual.ends_with(":2-3"),
        "expected :2-3, got {ref_visual}"
    );
}

#[test]
fn test_the_menu_selects_all_by_its_own_keys_and_insert_mode_pastes_through_the_window() {
    let tmp = TempDir::new("menu-select-all");
    let mut editor = tmp.editor("one\ntwo\nthree\n");
    press(&mut editor, "j");
    choose(&mut editor, "Select All");
    let selection = editor.selection().expect("a selection");
    assert_eq!((selection.start.0, selection.end.0), (0, 2), "every line");
    assert!(selection.linewise);

    editor.on_key(&key(KeyCode::Escape));
    press(&mut editor, "i");
    let paste = editor
        .context_items()
        .into_iter()
        .find(|item| item.label == "Paste")
        .expect("Paste while typing");
    assert_eq!(
        paste.run,
        winter_model::editor::MenuRun::Command("paste_from_clipboard"),
        "not a typed p"
    );
    let labels = menu_labels(&editor);
    assert!(
        !labels.contains(&"Go to Definition".to_string()),
        "no server, no server entries: {labels:?}"
    );
}
