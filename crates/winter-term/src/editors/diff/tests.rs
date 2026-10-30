//! A diff session driven by its keys.

use super::*;
use crate::model::editor::{DiffMerge, DiffText};
use crate::model::paint::row_text;

fn text(label: &str, body: &str, path: Option<PathBuf>) -> DiffText {
    DiffText {
        label: label.to_string(),
        lines: body.lines().map(str::to_string).collect(),
        path,
        shape: None,
    }
}

fn press(editor: &mut DiffEditor, keys: &str) {
    for c in keys.chars() {
        let key = match c {
            ' ' => Key::plain(KeyCode::Space),
            c => Key::plain(KeyCode::Char(c)),
        };
        editor.on_key(&key);
    }
}

fn ctrl(c: char) -> Key {
    Key {
        ctrl: true,
        ..Key::plain(KeyCode::Char(c))
    }
}

fn compare(a: &str, b: &str) -> DiffEditor {
    DiffEditor::new(DiffRequest {
        ancestor: None,
        merge: None,
        texts: vec![text("a", a, None), text("b", b, None)],
    })
}

#[test]
fn test_regions_are_stepped_through_and_copied_either_way() {
    let mut editor = compare("one\ntwo\nthree\nfour\n", "one\nTWO\nthree\nFOUR\nfive\n");
    assert_eq!(editor.status().segments[0], "region 1 of 2");
    press(&mut editor, "a");
    assert_eq!(editor.session.lines(B), ["one", "two", "three", "FOUR", "five"]);
    press(&mut editor, "n");
    assert_eq!(editor.status().segments[0], "region 2 of 2");
    press(&mut editor, "b");
    assert_eq!(editor.session.lines(A), ["one", "two", "three", "FOUR", "five"]);
    press(&mut editor, "p");
    press(&mut editor, "rb");
    assert_eq!(editor.session.lines(B)[1], "TWO", "the copy was put back");
    press(&mut editor, "!");
    assert_eq!(editor.status().segments[0], "region 1 of 1");
}

#[test]
fn test_a_scroll_chord_held_with_shift_too_is_left_to_the_window() {
    let mut editor = compare("one\n", "two\n");
    let ctrl_shift_d = Key {
        shift: true,
        ..ctrl('D')
    };
    assert_eq!(editor.on_key(&ctrl_shift_d), EditorOutcome::Ignored);
    assert_eq!(editor.on_key(&ctrl('d')), EditorOutcome::Consumed);
}

#[test]
fn test_whitespace_can_be_left_out_and_the_texts_swapped() {
    let mut editor = compare("a  b\nc\n", "a b\nC\n");
    assert_eq!(editor.session.regions.len(), 1, "the lines run together into one region");
    press(&mut editor, "##");
    assert_eq!(editor.session.regions.len(), 1);
    assert_eq!(editor.session.regions[0].lines[A], 1..2, "only the case change is left");
    press(&mut editor, "#c");
    assert!(editor.session.regions.is_empty());
    press(&mut editor, "~");
    assert_eq!(editor.session.sides[A].label, "b");
}

#[test]
fn test_columns_show_each_text_with_filler_and_the_current_region_marked() {
    let mut editor = compare("x\nsame\n", "y\nnew\nsame\n");
    let frame = editor.content(6, 41, false);
    let rows: Vec<String> = frame.rows.iter().map(row_text).collect();
    assert!(rows[0].contains(" A  a") && rows[0].contains(" B  b"), "{rows:?}");
    assert!(rows[1].contains("▶") && rows[1].contains('x') && rows[1].contains('y'));
    assert!(rows[2].contains('╱') && rows[2].contains("new"), "A has no line here: {}", rows[2]);
    assert!(rows.iter().all(|row| unicode_width::UnicodeWidthStr::width(row.as_str()) <= 41));
    press(&mut editor, "|");
    let bands: Vec<String> = editor.content(10, 30, false).rows.iter().map(row_text).collect();
    assert!(bands[0].starts_with(" A") && bands[5].starts_with(" B"), "{bands:?}");
}

#[test]
fn test_a_merge_keeps_a_side_or_both_and_writes_its_result() {
    let dir = std::env::temp_dir().join(format!("winter-diff-merge-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let output = dir.join("out.txt");
    let mut editor = DiffEditor::new(DiffRequest {
        ancestor: Some(text("base", "a\nb\nc\nd\n", None)),
        merge: Some(DiffMerge { output: Some(output.clone()) }),
        texts: vec![text("ours", "a\nX\nc\nD1\n", None), text("theirs", "a\nY\nc\nD2\n", None)],
    });
    press(&mut editor, "$$");
    press(&mut editor, "b");
    assert_eq!(editor.session.lines(C)[1], "Y");
    assert_eq!(editor.status().segments[0], "region 2 of 2", "on to the next");
    press(&mut editor, "+");
    assert!(editor.session.lines(C).contains(&model::OURS_MARK.to_string()));
    press(&mut editor, "pwc");
    let written = std::fs::read_to_string(&output).expect("written");
    assert!(written.starts_with("a\nY\nc\n<<<<<<<"), "{written}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_text_is_edited_in_place_and_the_regions_follow() {
    let mut editor = compare("one\ntwo\n", "one\nTWO\n");
    press(&mut editor, "eb");
    assert!(editor.status().mode.is_some(), "editing has a mode");
    press(&mut editor, "ciw");
    press(&mut editor, "two");
    editor.on_key(&Key::plain(KeyCode::Escape));
    editor.on_key(&ctrl('c'));
    editor.on_key(&ctrl('c'));
    assert_eq!(editor.status().mode, None, "back to the regions");
    assert!(editor.session.regions.is_empty(), "the texts now agree: {:?}", editor.session.lines(B));
    assert!(editor.is_dirty() || editor.session.sides[B].path.is_none());
    assert_eq!(press_quit(&mut editor), EditorOutcome::Close, "nothing to write: no file");
}

#[test]
fn test_ctrl_shift_c_stops_editing_in_place_mid_typing() {
    let mut editor = compare("one\ntwo\n", "one\nTWO\n");
    press(&mut editor, "eb");
    press(&mut editor, "ciw");
    press(&mut editor, "two");
    let chord = Key {
        shift: true,
        ..ctrl('C')
    };
    editor.on_key(&chord);
    assert_eq!(editor.status().mode, None, "back to the regions, from Insert too");
    assert!(editor.session.regions.is_empty(), "the texts now agree");
}

fn press_quit(editor: &mut DiffEditor) -> EditorOutcome {
    editor.on_key(&Key::plain(KeyCode::Char('q')))
}

#[test]
fn test_three_texts_copy_between_any_two() {
    let mut editor = DiffEditor::new(DiffRequest {
        ancestor: None,
        merge: None,
        texts: vec![text("a", "1\nA\n3\n", None), text("b", "1\nB\n3\n", None), text("c", "1\nC\n3\n", None)],
    });
    press(&mut editor, "ca");
    assert_eq!(editor.session.lines(A)[1], "C");
    press(&mut editor, "bc");
    assert_eq!(editor.session.lines(C)[1], "B");
}

#[test]
fn test_two_output_diffs_of_like_named_files_keep_their_own_copies() {
    // Each side's copy was named for its file alone, so a second diff of
    // files with the same name wrote over the first's before git read it.
    let named = |body: &str| {
        DiffEditor::new(DiffRequest {
            ancestor: None,
            merge: None,
            texts: vec![
                text("a", body, Some(PathBuf::from("/one/main.rs"))),
                text("b", "other\n", Some(PathBuf::from("/two/main.rs"))),
            ],
        })
    };
    let copies_of = |outcome: EditorOutcome| match outcome {
        EditorOutcome::Compare(request) => (request.old, request.new),
        other => panic!("not a comparison: {other:?}"),
    };
    let (first_old, first_new) = copies_of(named("first\n").diff_output());
    let (second_old, second_new) = copies_of(named("second\n").diff_output());
    assert_ne!(first_old, second_old);
    assert_ne!(first_new, second_new);
    assert_eq!(std::fs::read_to_string(&first_old).expect("first copy"), "first\n");
    let dir = first_old.parent().map(std::path::Path::to_path_buf);
    for copy in [first_old, first_new, second_old, second_new] {
        let _ = std::fs::remove_file(copy);
    }
    // The folder too, once empty: left, every run of the suite added one.
    if let Some(dir) = dir {
        let _ = std::fs::remove_dir(dir);
    }
}
