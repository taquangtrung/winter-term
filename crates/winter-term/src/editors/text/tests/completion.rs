//! The text editor's tests: the completion popup and snippets.

use super::*;

#[test]
fn test_typing_a_long_enough_word_opens_the_popup_and_tab_takes_its_entry() {
    let tmp = TempDir::new("complete-auto");
    let mut editor = tmp.editor("candidate canopy\n\n");
    press(&mut editor, "ji");
    press(&mut editor, "ca");
    assert!(editor.popups().completion.is_none(), "two letters are not enough");
    press(&mut editor, "n");
    assert!(editor.popups().completion.is_none(), "not while typing goes on");
    pause(&mut editor);
    let popup = editor.popups().completion.expect("open at three, once typing pauses");
    let labels: Vec<&str> = popup.items.iter().map(|item| item.label.as_str()).collect();
    assert_eq!(labels, ["candidate", "canopy"]);
    assert_eq!(popup.typed, 3);
    press_ctrl(&mut editor, 'n');
    assert_eq!(editor.popups().completion.map(|p| p.selected), Some(1));
    editor.on_key(&key(KeyCode::Tab));
    assert_eq!(editor.buffer().lines()[1], "canopy");
    assert!(editor.popups().completion.is_none());
    assert!(editor.modal.is_typing(), "still typing after it");
}

#[test]
fn test_the_popup_waits_for_typing_to_pause_and_a_key_off_the_word_calls_it_off() {
    let tmp = TempDir::new("complete-delay");
    let mut editor = tmp.editor("candidate\n\n");
    press(&mut editor, "jican");
    let first_due = editor.next_tick().expect("the popup is waited on");
    press(&mut editor, "d");
    let due = editor.next_tick().expect("still waited on");
    assert!(due >= first_due, "each key typed puts it off");
    assert_eq!(editor.take_cancelled_tick(), Some(first_due), "and calls the old deadline off");
    assert_eq!(editor.take_cancelled_tick(), None, "said once");
    press(&mut editor, " ");
    pause(&mut editor);
    assert!(editor.popups().completion.is_none(), "a space ended the word before the pause");
    // Set to nothing, it opens on the key, as before there was a pause.
    editor.take_completion_config(&crate::config::CompletionConfig {
        delay: Duration::ZERO,
        ..Default::default()
    });
    press(&mut editor, "can");
    assert!(editor.popups().completion.is_some(), "open at once");
}

#[test]
fn test_return_makes_a_new_line_unless_the_popup_was_asked_for_or_moved_through() {
    let tmp = TempDir::new("complete-return");
    let mut editor = tmp.editor("candidate\n\n");
    press(&mut editor, "jican");
    pause(&mut editor);
    assert!(editor.popups().completion.is_some());
    editor.on_key(&key(KeyCode::Enter));
    assert_eq!(editor.buffer().lines()[1], "can", "a new line, not the entry");
    press(&mut editor, "cand");
    editor.on_key(&ctrl_space());
    assert!(editor.popups().completion.is_some(), "asked for");
    editor.on_key(&key(KeyCode::Enter));
    assert_eq!(editor.buffer().lines()[2], "candidate");
}

#[test]
fn test_a_snippet_expands_with_the_cursor_on_its_first_field() {
    let tmp = TempDir::new("complete-snippet");
    let path = tmp.file("lib.rs", "\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    press(&mut editor, "i");
    press(&mut editor, "fn");
    editor.on_key(&ctrl_space());
    let popup = editor.popups().completion.expect("snippets of rust");
    assert_eq!(popup.items[0].label, "fn");
    editor.on_key(&key(KeyCode::Tab));
    assert_eq!(editor.buffer().lines()[..3], ["fn name() {", "    ", "}"].map(String::from));
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (0, 3), "on the name");
}

#[test]
fn test_escape_closes_the_popup_and_keeps_typing() {
    let tmp = TempDir::new("complete-escape");
    let mut editor = tmp.editor("candidate\n\n");
    press(&mut editor, "jican");
    pause(&mut editor);
    assert!(editor.popups().completion.is_some());
    editor.on_key(&key(KeyCode::Escape));
    assert!(editor.popups().completion.is_none());
    assert!(editor.modal.is_typing(), "the first Escape only closes it");
}
