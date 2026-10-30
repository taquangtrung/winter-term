//! The text editor's tests: conflict markers, and comparing the buffer with something else.

use super::*;

#[test]
fn test_the_conflicts_kept_between_frames_follow_an_edit() {
    // Found once per revision now, rather than on every paint: an edit that
    // breaks the block must still be seen.
    let tmp = TempDir::new("conflict-cache");
    let mut editor = tmp.editor("<<<<<<< ours\na\n=======\nb\n>>>>>>> theirs\n");
    assert_eq!(editor.doc().conflicts().len(), 1);
    assert_eq!(editor.doc().conflicts().len(), 1, "asked again, the same");
    press(&mut editor, "dd");
    assert!(editor.doc().conflicts().is_empty(), "no opening marker, no conflict");
}

#[test]
fn test_alt_n_steps_to_a_conflict_and_a_chord_keeps_one_side() {
    let tmp = TempDir::new("conflict-keep");
    let mut editor = tmp.editor(CONFLICTED);
    press_alt(&mut editor, KeyCode::Char('n'));
    assert_eq!(editor.buffer().row(), 1, "on the first conflict's marker");
    press_ctrl(&mut editor, 'c');
    press(&mut editor, "t");
    assert_eq!(
        editor.buffer().lines()[..3],
        ["a", "theirs", "b"].map(String::from)
    );
    assert_eq!(editor.buffer().row(), 1, "on what was kept");
    assert_eq!(
        editor.take_notice().map(|n| n.text),
        Some("1 conflict left".to_string())
    );
    press(&mut editor, "u");
    assert_eq!(editor.buffer().lines()[1], "<<<<<<< HEAD", "one undo takes it back");
}

#[test]
fn test_the_conflict_menu_offers_every_resolution_and_carries_out_the_one_chosen() {
    let tmp = TempDir::new("conflict-menu");
    let mut editor = tmp.editor(CONFLICTED);
    press(&mut editor, "j");
    let EditorOutcome::Pick(pick) = editor.run_command(MERGE_RESOLVE).expect("a command") else {
        panic!("a list to choose from");
    };
    assert!(
        !pick.items.iter().any(|item| item.contains("ancestor")),
        "no ancestor in a two-way conflict"
    );
    assert!(pick.items.iter().any(|item| item == "Keep both, theirs first"));
    editor.on_prompt(PromptReply {
        answer: Some("Keep ours in every conflict of the file".to_string()),
        tag: pick.tag,
    });
    assert_eq!(
        editor.buffer().lines()[..4],
        ["a", "ours", "b", "x"].map(String::from)
    );
    assert!(conflict::find(editor.buffer().lines()).is_empty());
}

#[test]
fn test_a_conflict_marker_is_painted_on_a_band_and_named_in_the_status() {
    let tmp = TempDir::new("conflict-paint");
    let mut editor = tmp.editor(CONFLICTED);
    let frame = editor.content(20, 60, false);
    let styles = |row: usize| -> Vec<Style> { frame.rows[row].iter().map(|span| span.style).collect() };
    assert!(styles(1).contains(&Style::Hunk), "the opening marker");
    assert!(!styles(2).contains(&Style::Hunk), "our side keeps its colors");
    assert!(editor
        .status()
        .segments
        .iter()
        .any(|segment| segment == "2 conflicts"));
}

#[test]
fn test_a_file_with_no_conflicts_has_no_conflict_keys() {
    let tmp = TempDir::new("conflict-none");
    let mut editor = tmp.editor("a\n=======\nb\n");
    assert!(!editor
        .keys()
        .iter()
        .any(|help| help.label == "Next merge conflict"));
    press_alt(&mut editor, KeyCode::Char('n'));
    assert_eq!(editor.buffer().row(), 0);
}

#[test]
fn test_comparing_with_disk_sets_the_file_against_the_buffer_as_it_stands() {
    let tmp = TempDir::new("compare-disk");
    let path = tmp.file("main.rs", "a\nb\n");
    let mut editor = TextEditor::new(path.clone(), None).expect("opens");
    press(&mut editor, "ddix");
    editor.on_key(&key(KeyCode::Escape));
    let EditorOutcome::Diff(request) = editor.run_command(DIFF_DISK).expect("a command") else {
        panic!("a comparison");
    };
    let [disk, buffer] = &request.texts[..] else {
        panic!("two texts");
    };
    assert_eq!((disk.lines.clone(), disk.path.clone()), (vec!["a".to_string(), "b".to_string()], None));
    assert_eq!((buffer.lines.clone(), buffer.path.clone()), (vec!["xb".to_string()], Some(path)));
}

#[test]
fn test_comparing_with_the_clipboard_asks_for_it_rather_than_pasting_it() {
    let tmp = TempDir::new("compare-clipboard");
    let mut editor = tmp.editor("a\n");
    assert_eq!(editor.run_command(DIFF_CLIPBOARD), Some(EditorOutcome::Paste));
    let EditorOutcome::Diff(request) = editor.on_paste("b\n".to_string()) else {
        panic!("a comparison");
    };
    assert_eq!(request.texts[0].lines, ["b"]);
    assert_eq!(editor.buffer().lines(), ["a"], "nothing was pasted");
    let pasted = editor.on_paste("c".to_string());
    assert!(!matches!(pasted, EditorOutcome::Diff(_)), "the next paste pastes");
}

#[test]
fn test_comparing_with_another_buffer_needs_one_open() {
    let tmp = TempDir::new("compare-buffer");
    let mut editor = tmp.editor("a\n");
    assert_eq!(editor.run_command(DIFF_BUFFER), Some(EditorOutcome::Consumed));
    // Another file open as text anywhere in the window, in an editor of its own.
    let other = tmp.file("b.txt", "b\n");
    let _other_editor = TextEditor::new(other.clone(), None).expect("opens");
    let EditorOutcome::Pick(pick) = editor.run_command(DIFF_BUFFER).expect("a command") else {
        panic!("a list of the other buffers");
    };
    assert_eq!(pick.items.len(), 1, "every open file but this one");
    let EditorOutcome::Diff(request) = editor.on_prompt(PromptReply {
        answer: Some(pick.items[0].clone()),
        tag: pick.tag,
    }) else {
        panic!("a comparison");
    };
    let other = other.canonicalize().expect("a real path");
    assert_eq!(request.texts[0].path, Some(other), "the one chosen, written back to its file");
    assert_eq!(request.texts[1].path, Some(tmp.0.join("a.txt")), "and this one");
}
