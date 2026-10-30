//! The text editor's tests: what the editor leaves to the window, and servers' edits elsewhere.

use super::*;

#[test]
fn test_the_alt_chords_the_window_keeps_still_reach_it() {
    // `M-h` and `M-l` move focus, and `M-1` picks a pane; claiming
    // them for editing would strand the window chords inside a message.
    let tmp = TempDir::new("emacs-window");
    let mut editor = tmp.editor("abc\n");
    press(&mut editor, "i");
    for c in ['h', 'j', 'k', 'l', '1'] {
        assert_eq!(
            press_alt(&mut editor, KeyCode::Char(c)),
            EditorOutcome::Ignored,
            "{c}"
        );
    }
    assert_eq!(editor.buffer().lines(), ["abc"]);
}

#[test]
fn test_a_second_file_is_the_windows_to_open_and_the_editor_keeps_its_own() {
    let tmp = TempDir::new("buffers");
    let mut editor = tmp.editor("one\n");
    let other = tmp.file("b.txt", "two\n");

    editor.open(other.clone(), Some(3));
    assert_eq!(editor.buffer().lines(), ["one"], "still its own file");
    assert_eq!(
        editor.take_requests(),
        [EditorRequest::Open(OpenTarget {
            column: None,
            line: Some(3),
            path: other,
        })],
        "the window is asked to show the other"
    );
    assert!(editor.take_requests().is_empty(), "and asked once");
}

#[test]
fn test_the_buffer_keys_are_the_windows() {
    let tmp = TempDir::new("buffer-keys");
    let mut editor = tmp.editor("one\n");
    assert_eq!(
        editor.on_key(&key(KeyCode::Char(']'))),
        EditorOutcome::Consumed,
        "half a chord"
    );
    assert_eq!(
        editor.on_key(&key(KeyCode::Char('b'))),
        EditorOutcome::RunActionHere("next_tab".to_string())
    );
}

#[test]
fn test_edits_a_server_makes_to_another_file_are_the_windows_to_make() {
    let tmp = TempDir::new("foreign-edits");
    let mut editor = tmp.editor("one\n");
    let other = tmp.file("b.txt", "two\n");
    let edit = winter_lsp::protocol::TextEdit {
        new_text: "2".to_string(),
        range: winter_lsp::protocol::Range::default(),
    };
    editor.apply_file_edits(&vec![(other.clone(), vec![edit.clone()])]);
    assert_eq!(
        editor.take_requests(),
        [EditorRequest::Edit {
            edits: vec![edit],
            path: other,
        }]
    );
}

#[cfg(unix)]
#[test]
fn test_a_server_s_edit_to_a_file_opened_through_a_link_is_made_in_it() {
    // A server names the file the link leads to: compared as written, the
    // edit was sent off to the window as another file's.
    let tmp = TempDir::new("linked-edits");
    let real = tmp.file("real.txt", "one\n");
    let link = tmp.0.join("link.txt");
    std::os::unix::fs::symlink(&real, &link).expect("link");
    let mut editor = TextEditor::new(link, None).expect("opens");
    let edit = winter_lsp::protocol::TextEdit {
        new_text: "1".to_string(),
        range: winter_lsp::protocol::Range {
            end: winter_lsp::protocol::Position { character: 3, line: 0 },
            start: winter_lsp::protocol::Position::default(),
        },
    };
    editor.apply_file_edits(&vec![(real, vec![edit])]);
    assert!(editor.take_requests().is_empty(), "made here, not handed on");
    assert_eq!(editor.doc().buffer.lines()[0], "1");
}

#[test]
fn test_the_window_chords_still_reach_the_window_from_either_mode() {
    // An Alt chord read as text types an `h` instead of moving focus, and
    // in Normal mode `M-l` would move a character: an editor that swallows
    // the window chords traps the user inside it. (`M-n` and `M-p` are the
    // editor's own, through the occurrences of a word.)
    let tmp = TempDir::new("alt");
    let alt = |c: char| Key {
        alt: true,
        code: KeyCode::Char(c),
        ctrl: false,
        shift: false,
        sup: false,
    };

    let mut editor = tmp.editor("one\ntwo\n");
    assert_eq!(editor.on_key(&alt('h')), EditorOutcome::Ignored);
    press(&mut editor, "dd");
    assert_eq!(editor.on_key(&alt('l')), EditorOutcome::Ignored);
    assert_eq!(editor.buffer().lines(), ["two"], "nothing was changed");

    press(&mut editor, "i");
    assert_eq!(editor.on_key(&alt('h')), EditorOutcome::Ignored);
    assert_eq!(editor.buffer().lines(), ["two"], "and nothing was typed");

    // The two buffer ends the shared layer claims are still the editor's.
    let mut ends = tmp.editor("one\ntwo\nthree\n");
    let shifted = Key {
        alt: true,
        code: KeyCode::Char('>'),
        ctrl: false,
        shift: true,
        sup: false,
    };
    assert_eq!(ends.on_key(&shifted), EditorOutcome::Consumed);
    assert_eq!(ends.buffer().row(), 2);
}

#[test]
fn test_editor_jump_targets_preview_confirm_and_cancel() {
    let tmp = TempDir::new("editor_jump");
    let mut editor = tmp.editor("first line\n\nthird line\nfourth line\n");
    let targets = editor.jump_targets().expect("jump targets");
    assert_eq!(targets.len(), 3);
    assert_eq!(targets[0].target, 0);
    assert_eq!(targets[1].target, 2);
    assert_eq!(targets[2].target, 3);
    assert_eq!(editor.jump_cursor(), Some(0));

    // Live preview to line 2
    editor.on_jump(JumpStep::Preview(2));
    assert_eq!(editor.buffer().row(), 2);

    // Cancel restores cursor to line 0
    editor.on_jump(JumpStep::Cancel);
    assert_eq!(editor.buffer().row(), 0);

    // With painted viewport (e.g. 10 rows), preview centers at viewport / 2
    editor.content(10, 40, false);
    editor.on_jump(JumpStep::Preview(3));
    assert_eq!(editor.buffer().row(), 3);
    assert_eq!(editor.doc().scroll, 3_usize.saturating_sub(10 / 2));

    // Preview to line 3 and confirm
    editor.on_jump(JumpStep::Confirm(3));
    assert_eq!(editor.buffer().row(), 3);
    assert_eq!(editor.doc().scroll, 3_usize.saturating_sub(10 / 2));
    assert_eq!(editor.doc().marks.jump, Some((0, 0)));
}

#[test]
fn test_a_capital_as_the_window_sends_it_is_the_capital_everywhere() {
    // Regression: the canonical capital (`h` with Shift) matched no arm
    // written for `H`, so `H` over wrapped lines and a capital typed into
    // every occurrence at once went astray.
    let tmp = TempDir::new("canonical-capitals");
    let text: String = (0..200).map(|n| format!("count {n} count\n")).collect();
    let mut editor = tmp.editor(&text);
    editor.content(20, 40, true);
    press(&mut editor, "5j");
    editor.on_key(&shifted('h'));
    assert_eq!(editor.buffer().row(), 0, "H goes to the top screen row");
    editor.on_key(&ctrl(';'));
    editor.on_key(&shifted('z'));
    assert!(editor.buffer().lines()[0].contains("Zcount"), "{}", editor.buffer().lines()[0]);
}
