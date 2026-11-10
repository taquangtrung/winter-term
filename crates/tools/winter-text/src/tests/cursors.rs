use super::*;

/// The editor's text as the file would be written, each line ending in a
/// newline.
fn text(editor: &TextEditor) -> String {
    format!("{}\n", editor.buffer().lines().join("\n"))
}

fn run(editor: &mut TextEditor, command: &str) {
    editor
        .run_command(command)
        .expect("a command of the editor's");
}

#[test]
fn test_typing_goes_in_at_every_cursor_and_one_undo_takes_it_all_back() {
    let tmp = TempDir::new("cursors-typing");
    let mut editor = tmp.editor("foo\nfoo\nfoo\n");
    run(&mut editor, editor::CURSOR_ADD_BELOW);
    run(&mut editor, editor::CURSOR_ADD_BELOW);
    press(&mut editor, "iX-");
    editor.on_key(&key(KeyCode::Escape));
    assert_eq!(text(&editor), "X-foo\nX-foo\nX-foo\n");
    press(&mut editor, "u");
    assert_eq!(text(&editor), "foo\nfoo\nfoo\n", "one undo for the lot");
}

#[test]
fn test_an_operator_and_its_motion_apply_at_every_occurrence() {
    let tmp = TempDir::new("cursors-operator");
    let mut editor = tmp.editor("let total = total + total;\n");
    editor.buffer_mut().move_to(0, 4);
    run(&mut editor, editor::CURSOR_ADD_ALL);
    assert_eq!(
        editor.status().segments.first().map(String::as_str),
        Some("3 cursors")
    );
    press(&mut editor, "ciwsum");
    editor.on_key(&key(KeyCode::Escape));
    assert_eq!(text(&editor), "let sum = sum + sum;\n");
}

#[test]
fn test_two_cursors_on_one_line_each_type_where_they_stand() {
    let tmp = TempDir::new("cursors-one-line");
    let mut editor = tmp.editor("a b a\n");
    run(&mut editor, editor::CURSOR_ADD_NEXT);
    press(&mut editor, "ix");
    editor.on_key(&key(KeyCode::Escape));
    assert_eq!(text(&editor), "xa b xa\n");
}

#[test]
fn test_the_next_occurrence_is_after_the_cursor_added_last_and_skip_moves_it() {
    let tmp = TempDir::new("cursors-next");
    let mut editor = tmp.editor("x\nx\nx\nx\n");
    run(&mut editor, editor::CURSOR_ADD_NEXT);
    run(&mut editor, editor::CURSOR_SKIP);
    press(&mut editor, "rY");
    assert_eq!(text(&editor), "Y\nx\nY\nx\n", "the second x was skipped");
}

#[test]
fn test_escape_in_normal_mode_leaves_only_the_main_cursor() {
    let tmp = TempDir::new("cursors-escape");
    let mut editor = tmp.editor("ab\nab\n");
    run(&mut editor, editor::CURSOR_ADD_BELOW);
    editor.on_key(&key(KeyCode::Escape));
    press(&mut editor, "x");
    assert_eq!(text(&editor), "b\nab\n", "Esc put the other cursor away");
}

#[test]
fn test_each_cursor_puts_back_what_it_took_itself() {
    let tmp = TempDir::new("cursors-register");
    let mut editor = tmp.editor("one\ntwo\n");
    run(&mut editor, editor::CURSOR_ADD_BELOW);
    press(&mut editor, "dw");
    assert_eq!(text(&editor), "\n\n");
    press(&mut editor, "p");
    assert_eq!(text(&editor), "one\ntwo\n");
}

#[test]
fn test_each_cursor_selects_on_its_own_and_an_operator_takes_each_selection() {
    let tmp = TempDir::new("cursors-select");
    let mut editor = tmp.editor("foo bar\nfoo bar\n");
    run(&mut editor, editor::CURSOR_ADD_BELOW);
    press(&mut editor, "ve");
    assert_eq!(
        editor.cursor_selections().len(),
        1,
        "the other cursor selects too"
    );
    press(&mut editor, "d");
    assert_eq!(text(&editor), " bar\n bar\n");
}

#[test]
fn test_the_next_occurrence_of_a_selection_is_selected_at_its_cursor() {
    let tmp = TempDir::new("cursors-select-next");
    let mut editor = tmp.editor("ab x ab x\n");
    press(&mut editor, "vl");
    run(&mut editor, editor::CURSOR_ADD_NEXT);
    press(&mut editor, "cZ");
    editor.on_key(&key(KeyCode::Escape));
    assert_eq!(text(&editor), "Z x Z x\n");
}
