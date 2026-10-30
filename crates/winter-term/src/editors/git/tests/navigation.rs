//! The git view's tests: moving about the view, searching, and opening what a row names.

use super::*;

#[test]
fn test_a_chord_with_an_extra_modifier_is_left_to_the_window() {
    // `C-S-d` is the window's `dir_page`; it must not be read as
    // the `C-d` half page, and `C-M-n` is not `M-n`.
    let mut editor = loaded_editor();
    for key in [
        ctrl_shift('d'),
        ctrl_shift('u'),
        ctrl_shift('l'),
        ctrl_shift('o'),
        Key {
            ctrl: true,
            ..alt(KeyCode::Char('n'))
        },
    ] {
        assert_eq!(editor.on_key(&key), EditorOutcome::Ignored, "{key:?}");
    }
}

#[test]
fn test_the_address_is_the_repository_whatever_the_cursor_is_on() {
    // Before the repository is known there is nothing to name; afterwards
    // the title bar names the repository, the same text the `Repo:` row
    // carries, and not the folder of the file the cursor rests on.
    let mut editor = GitEditor::new(PathBuf::from("/repo/sub"));
    assert_eq!(editor.address(), None);
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    assert_eq!(editor.address().as_deref(), Some("/repo"));
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "staged.rs");
    assert_eq!(editor.address().as_deref(), Some("/repo"));
}

#[test]
fn test_the_wheel_moves_the_view_not_the_cursor_and_a_key_returns_to_it() {
    // Like scrolling an editor: the cursor stays on its row while the
    // view moves, hidden when it has left the screen, and the next key
    // brings the view back to it.
    let mut editor = loaded_editor();
    editor.list.set_cursor(0);
    editor.content(8, 80, false);
    assert_eq!(editor.on_scroll(-3), EditorOutcome::Consumed);
    let shown = editor.content(8, 80, false);
    assert_eq!(editor.list.view().scroll(), 3);
    assert_eq!(editor.list.cursor(), 0, "the cursor did not move");
    assert_eq!(shown.cursor_line, None, "and it is off screen");
    editor.on_scroll(3);
    editor.content(8, 80, false);
    assert_eq!(editor.list.view().scroll(), 0);
    editor.on_scroll(-3);
    editor.content(8, 80, false);
    editor.on_key(&press_char('j'));
    let shown = editor.content(8, 80, false);
    assert_eq!(
        editor.list.view().scroll(),
        editor.list.cursor(),
        "a key reveals the cursor at the edge"
    );
    assert!(shown.cursor_line.is_some());
}

#[test]
fn test_mouse_click_sets_cursor_position() {
    let mut editor = loaded_editor();
    editor.content(20, 80, false);
    assert_eq!(editor.list.cursor(), 0);

    // Clicking row 3 sets the cursor to row 3.
    let outcome = editor.on_mouse(Point {
        col: 5,
        drag: false,
        row: 3,
    });
    assert_eq!(outcome, EditorOutcome::Consumed);
    assert_eq!(editor.list.cursor(), 3);

    // A drag click is ignored so the window can run text selection across cells.
    let outcome = editor.on_mouse(Point {
        col: 5,
        drag: true,
        row: 1,
    });
    assert_eq!(outcome, EditorOutcome::Ignored);
    assert_eq!(editor.list.cursor(), 3, "drag did not move cursor");

    // Clicking beyond the end of the rows is ignored.
    let outcome = editor.on_mouse(Point {
        col: 5,
        drag: false,
        row: 50,
    });
    assert_eq!(outcome, EditorOutcome::Ignored);
    assert_eq!(editor.list.cursor(), 3);

    // Double-clicking on a file row opens it.
    let staged_row = editor
        .rows
        .iter()
        .position(|row| matches!(&row.item, Item::File(file) if file.path == "staged.rs"))
        .expect("staged.rs row");
    let outcome = editor.on_double_click(Point {
        col: 0,
        drag: false,
        row: staged_row,
    });
    assert_eq!(
        outcome,
        EditorOutcome::OpenPath(OpenTarget::file(PathBuf::from("/repo/staged.rs")))
    );
}

#[test]
fn test_mouse_click_in_output_view_sets_cursor_position() {
    let mut editor = loaded_editor();
    // Open a log output view.
    editor.on_job(reply(
        git_exec::TAG_READ,
        "commit 111111\ncommit 222222\ncommit 333333\n",
    ));
    assert!(editor.output.is_some());
    editor.content(20, 80, false);

    // Clicking the header row (row 0) is ignored.
    let outcome = editor.on_mouse(Point {
        col: 0,
        drag: false,
        row: 0,
    });
    assert_eq!(outcome, EditorOutcome::Ignored);

    // Clicking row 2 (which is content row 1, 0-indexed after header) selects line 1.
    let outcome = editor.on_mouse(Point {
        col: 0,
        drag: false,
        row: 2,
    });
    assert_eq!(outcome, EditorOutcome::Consumed);
    assert_eq!(editor.output.as_ref().unwrap().list.cursor(), 1);
}

#[test]
fn test_enter_in_a_blame_opens_the_file_at_the_line_under_the_cursor() {
    // A blame is one row per line of the file, so the row the cursor is
    // on is the line to land on: opening at the top would throw away the
    // only thing the reader was pointing at.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    editor.on_key(&alt(KeyCode::Char('b')));
    answer_job(
        &mut editor,
        git_exec::TAG_BLAME,
        "abc1234 (Someone 2026-09-18 1) fn main() {\n             abc1234 (Someone 2026-09-18 2)     old();\n             def5678 (Another 2026-09-18 3) }\n",
    );

    let output = editor.output.as_mut().expect("the blame");
    output.list.set_cursor(2);
    assert_eq!(
        editor.on_key(&press(KeyCode::Enter)),
        EditorOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/working.rs"), 3))
    );
}

#[test]
fn test_enter_on_a_row_belonging_to_no_file_is_left_alone() {
    // A log's rows stand for nothing to open, and swallowing the key
    // there would take it from the window for no gain.
    let mut editor = loaded_editor();
    editor.on_job(reply(
        git_exec::TAG_LOG,
        &log_output(&[("abc1234", "a commit")]),
    ));
    editor.on_key(&ctrl_shift('l'));
    if editor.output.is_some() {
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::Ignored
        );
    }

    // In the status view, a section heading names a set of files rather
    // than one, so Enter has nothing to open there either.
    let mut status = loaded_editor();
    let heading = status
        .rows
        .iter()
        .position(|row| matches!(row.item, Item::Heading(_)))
        .expect("a heading row");
    status.list.set_cursor(heading);
    assert_eq!(
        status.on_key(&press(KeyCode::Enter)),
        EditorOutcome::Consumed
    );
}

#[test]
fn test_wrapping_puts_the_cursor_on_its_wrapped_screen_row() {
    // A changed line wider than the pane takes several screen rows, so the
    // band must follow the wrapped offset of the cursor's row, not its
    // row number, or it lights up the wrong line.
    let long = "x".repeat(45);
    let text = format!(
        "diff --git a/working.rs b/working.rs\n--- a/working.rs\n+++ b/working.rs\n@@ -1 +1 @@\n-{long}\n+{long}y\n"
    );
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    editor.on_key(&press(KeyCode::Tab));
    answer_job(&mut editor, git_exec::TAG_DIFF, &text);
    let added = editor
        .rows
        .iter()
        .position(|row| row_text(&row.spans).contains(&format!("{long}y")))
        .expect("the added line");
    editor.list.set_cursor(added);
    // Stated relative to the cursor's row index: the status header is a
    // labelled block whose height can change, and an absolute screen row
    // here would be measuring the header rather than the wrapping.
    let flat = editor.content(20, 80, false);
    assert_eq!(
        flat.cursor_line,
        Some(added),
        "unwrapped, every row is one screen row, so the band sits on its own index"
    );
    let wrapped = editor.content(20, 32, true);
    assert_eq!(
        wrapped.cursor_line,
        Some(added + 2),
        "the wrapped header and removed line's wrapped rows push the band down"
    );
    assert!(
        wrapped.wrap_indents.contains(&1),
        "the hunk's lines carry their marker column, got {:?}",
        wrapped.wrap_indents
    );
}

#[test]
fn test_the_emacs_buffer_ends_reach_both_ends_of_either_view() {
    // `M->` and `M-<` land on the last and first row, in the status view
    // and in an output view alike: both are read top to bottom, and the
    // Vim `G`/`gg` pair is otherwise the only way there.
    let mut editor = loaded_editor();
    editor.list.set_cursor(0);
    editor.on_key(&Key {
        shift: true,
        ..alt_char('>')
    });
    assert_eq!(editor.list.cursor(), editor.rows.len() - 1, "the last row");
    editor.on_key(&Key {
        shift: true,
        ..alt_char('<')
    });
    assert_eq!(editor.list.cursor(), 0, "and back to the first");

    editor.on_job(reply(git_exec::TAG_SHOW, SHOWN_TWO_FILES));
    editor.on_key(&Key {
        shift: true,
        ..alt_char('>')
    });
    let output = editor.output.as_ref().expect("the commit view");
    assert_eq!(output.list.cursor(), output.lines.len() - 1);
    editor.on_key(&Key {
        shift: true,
        ..alt_char('<')
    });
    assert_eq!(editor.output.as_ref().unwrap().list.cursor(), 0);
}

#[test]
fn test_the_motions_along_a_row_are_left_to_the_window_s_column_cursor() {
    // `w`, `e`, `0`, and `$` move along the row the cursor is on, which the
    // window does for every editor: the view declines them and its cursor
    // stays on its row.
    let mut editor = loaded_editor();
    editor.list.set_cursor(2);
    for key in ['w', 'e', 'W', 'E', '0', '$', '^', 'h', 'l'] {
        assert_eq!(
            editor.on_key(&press(KeyCode::Char(key))),
            EditorOutcome::Ignored,
            "{key}"
        );
    }
    assert_eq!(editor.list.cursor(), 2);
}

#[test]
fn test_the_braces_jump_between_section_headings() {
    let mut editor = loaded_editor();
    editor.on_key(&press(KeyCode::Char('}')));
    assert!(matches!(
        editor.selected().map(|row| &row.item),
        Some(Item::Heading(_) | Item::RecentHeading)
    ));
    let first = editor.list.cursor();
    editor.on_key(&press(KeyCode::Char('}')));
    assert!(
        editor.list.cursor() > first,
        "each brace reaches the next heading"
    );
    editor.on_key(&press(KeyCode::Char('{')));
    assert_eq!(editor.list.cursor(), first, "and back to it");
}

#[test]
fn test_gg_and_the_paging_motions_work_over_the_view() {
    let mut editor = loaded_editor();
    editor.on_key(&press(KeyCode::Char('g')));
    editor.on_key(&press(KeyCode::Char('g')));
    assert_eq!(
        editor.list.cursor(),
        0,
        "gg through the view's own g leader"
    );

    editor.on_key(&press(KeyCode::End));
    assert_eq!(
        editor.list.cursor(),
        editor.rows.len() - 1,
        "End is the last row"
    );

    editor.on_key(&press(KeyCode::Home));
    assert_eq!(editor.list.cursor(), 0);
    editor.content(10, 80, false);
    editor.on_key(&ctrl(KeyCode::Char('d')));
    assert_eq!(editor.list.cursor(), 4, "half the viewport down");
    editor.on_key(&ctrl(KeyCode::Char('u')));
    assert_eq!(editor.list.cursor(), 0, "and back up");
}

#[test]
fn test_the_magit_keys_sit_where_evil_collection_puts_them() {
    // `G` is the bottom, as in every other editor; magit's letters that Vim
    // leaves a list of rows no use for are magit's, and the rest moved as
    // evil-collection moves them.
    let mut editor = loaded_editor();
    editor.on_key(&press(KeyCode::Char('G')));
    assert_eq!(editor.list.cursor(), editor.rows.len() - 1);
    editor.on_key(&press_char('g'));
    assert!(matches!(editor.on_key(&press_char('r')), EditorOutcome::Job(_)), "gr re-reads");
    for (key, popup) in [
        (press_char('b'), Popup::Branch),
        (press_char('B'), Popup::Bisect),
        (press_char('M'), Popup::Remote),
        (press_char('_'), Popup::Revert),
        (press_char('Z'), Popup::Stash),
        (press_char('%'), Popup::Worktree),
        (press_char('y'), Popup::Yank),
        (press_char('z'), Popup::Fold),
        (press_char('g'), Popup::Jump),
        (ctrl(KeyCode::Char('l')), Popup::Log),
    ] {
        let mut editor = loaded_editor();
        editor.on_key(&key);
        assert_eq!(editor.menus.popup, Some(popup), "{key:?}");
    }
}

#[test]
fn test_the_g_keys_jump_between_sections() {
    let mut editor = loaded_editor();
    editor.on_key(&press(KeyCode::Char('g')));
    editor.on_key(&press(KeyCode::Char('s')));
    assert_eq!(
        editor.selected().map(|row| &row.item),
        Some(&Item::Heading(Section::Staged))
    );

    editor.on_key(&press(KeyCode::Char('g')));
    editor.on_key(&press(KeyCode::Char('n')));
    assert_eq!(
        editor.selected().map(|row| &row.item),
        Some(&Item::Heading(Section::Untracked))
    );
}

#[test]
fn test_entity_motion_skips_the_blank_lines_between_sections() {
    // Plain `j` walks through the gaps; `]` is what steps from one thing
    // to the next thing.
    let mut editor = loaded_editor();
    editor.list.set_cursor(0);
    editor.on_key(&press_char(']'));
    assert_ne!(editor.selected().map(|row| &row.item), Some(&Item::None));
}

#[test]
fn test_a_search_moves_the_cursor_to_the_row_holding_the_text() {
    let mut editor = loaded_editor();
    search(&mut editor, "working");
    assert!(row_under_cursor(&editor).contains("working.rs"));
}

#[test]
fn test_the_repeat_keys_step_through_the_matches_in_both_directions() {
    let mut editor = loaded_editor();
    search(&mut editor, ".rs");
    let first = row_under_cursor(&editor);

    editor.on_key(&press_char('n'));
    let second = row_under_cursor(&editor);
    assert_ne!(first, second, "the next match is a different row");

    editor.on_key(&press_char('N'));
    assert_eq!(row_under_cursor(&editor), first, "and back again");
}

#[test]
fn test_a_search_walks_the_output_view_while_one_is_up() {
    // The status rows are not on screen here, so searching them would move a
    // cursor nobody can see and leave the visible one where it was.
    let mut editor = loaded_editor();
    choose(&mut editor, 'l', 'l');
    editor.on_job(reply(
        git_exec::TAG_LOG_VIEW,
        &log_output(&[("aaa", "one"), ("bbb", "two"), ("ccc", "three")]),
    ));
    let before = editor.list.cursor();

    search(&mut editor, "ccc");
    // Located rather than hardcoded: the log view heads its list, and each
    // commit takes an identity row and an authorship row, so a literal index
    // here would only re-encode the current layout.
    let expected = editor
        .output
        .as_ref()
        .and_then(|output| output.lines.iter().position(|line| line.contains("ccc")));
    assert!(expected.is_some(), "the log lists the commit searched for");
    assert_eq!(
        editor.output.as_ref().map(|output| output.list.cursor()),
        expected
    );
    assert_eq!(editor.list.cursor(), before);
}

#[test]
fn test_a_search_that_finds_nothing_reports_it_instead_of_going_quiet() {
    let mut editor = loaded_editor();
    editor.on_key(&press_char('n'));
    assert_eq!(editor.take_notice(), Some(Notice::error("no search")));

    search(&mut editor, "absent");
    assert_eq!(
        editor.take_notice(),
        Some(Notice::error("not found: absent"))
    );
}

#[test]
fn test_blame_runs_against_the_file_under_the_cursor() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    let command = command_of(&editor.on_key(&alt_char('b')));
    assert_eq!(command.tag, git_exec::TAG_BLAME);
    assert_eq!(command.args.last().map(String::as_str), Some("working.rs"));
    assert_eq!(command.cwd, PathBuf::from("/repo"));
}

#[test]
fn test_blame_off_a_file_says_there_is_nothing_to_blame() {
    // A heading and a commit have no single path, and running blame on the
    // repository root would report a git error the user did not ask for.
    let mut editor = loaded_editor();
    editor.list.set_cursor(0);
    assert_eq!(editor.on_key(&alt_char('b')), EditorOutcome::Consumed);
    assert_eq!(
        notice_text(&mut editor).as_deref(),
        Some("no file to blame here")
    );
}

#[test]
fn test_keys_before_the_root_is_known_do_nothing() {
    // The view is up as soon as the key is pressed, so every command has to
    // survive being asked before git has answered.
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    for code in [
        KeyCode::Char('s'),
        KeyCode::Char('u'),
        KeyCode::Char('P'),
        KeyCode::Char('F'),
        KeyCode::Char('f'),
    ] {
        assert_eq!(editor.on_key(&press(code)), EditorOutcome::Consumed);
    }
}

#[test]
fn test_git_jump_status_rows_and_output_lines() {
    let mut editor = loaded_editor();
    let targets = editor.jump_targets().expect("status jump targets");
    assert!(!targets.is_empty());
    assert_eq!(editor.jump_cursor(), Some(0));

    // Jump preview on status view
    editor.list.view_mut().set_rows(10);
    editor.on_jump(JumpStep::Preview(1));
    assert_eq!(editor.list.cursor(), 1);
    assert_eq!(editor.list.view().scroll(), 1_usize.saturating_sub(10 / 2));
    editor.on_jump(JumpStep::Cancel);
    assert_eq!(editor.list.cursor(), 0);
    assert_eq!(editor.list.view().scroll(), 0);

    // Jump preview and confirm on status view
    editor.on_jump(JumpStep::Preview(1));
    editor.on_jump(JumpStep::Confirm(1));
    assert_eq!(editor.list.cursor(), 1);
    assert_eq!(editor.list.view().scroll(), 1_usize.saturating_sub(10 / 2));
    assert!(!editor.list.is_jumping());

    // Now with output (e.g. blame)
    editor.on_job(reply(git_exec::TAG_BLAME, "line 1\nline 2\nline 3\n"));
    let output_targets = editor.jump_targets().expect("output jump targets");
    assert_eq!(output_targets.len(), 3);
    assert_eq!(editor.jump_cursor(), Some(0));

    editor.on_jump(JumpStep::Preview(2));
    assert_eq!(editor.output.as_ref().unwrap().list.cursor(), 2);
    editor.on_jump(JumpStep::Cancel);
    assert_eq!(editor.output.as_ref().unwrap().list.cursor(), 0);

    editor.on_jump(JumpStep::Preview(2));
    editor.on_jump(JumpStep::Confirm(2));
    assert_eq!(editor.output.as_ref().unwrap().list.cursor(), 2);
}

#[test]
fn test_a_re_read_keeps_the_cursor_on_its_row() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    let moved = STATUS_OUTPUT.replace("? new.rs", "? aaa.rs\n? new.rs");
    editor.on_job(reply(git_exec::TAG_STATUS, &moved));
    assert!(
        matches!(&editor.selected().expect("a row").item, Item::File(file) if file.path == "working.rs"),
        "still on working.rs"
    );
}

#[test]
fn test_every_key_of_the_view_is_spelled_right() {
    use crate::model::input::table;
    for keys in [keys::STATUS_KEYS, keys::OUTPUT_KEYS, keys::SEARCH_KEYS] {
        assert_eq!(table::invalid_specs(keys), Vec::<&str>::new());
    }
}

#[test]
fn test_git_show_scopes_inspects_item() {
    let mut editor = GitEditor::new(PathBuf::from("/tmp"));
    assert!(editor.token_inspect().is_none());

    let outcome = editor.run_command(SHOW_SCOPES);
    assert_eq!(outcome, Some(EditorOutcome::Consumed));

    let data = editor.token_inspect().expect("inspect data active");
    assert_eq!(data.language, "Git");
    assert!(data.scopes.contains(&"source.git".to_string()));

    // Escape dismisses inspection
    let esc = editor.on_key(&crate::model::input::Key::plain(KeyCode::Escape));
    assert_eq!(esc, EditorOutcome::Consumed);
    assert!(editor.token_inspect().is_none());
}

#[test]
fn test_an_output_view_takes_the_shared_motions_between_rows() {
    // A commit, a log, or a blame reads like every other view: `G` and `gg`
    // reach its ends, and the motions along a row are left to the window.
    let mut editor = loaded_editor();
    editor.on_job(reply(git_exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    editor.on_key(&press(KeyCode::Tab));
    editor.on_key(&press_char('G'));
    let output = editor.output.as_ref().expect("the commit");
    assert_eq!(output.list.cursor(), output.rows.len() - 1);
    editor.on_key(&press_char('g'));
    editor.on_key(&press_char('g'));
    assert_eq!(editor.output.as_ref().unwrap().list.cursor(), 0);
    assert_eq!(editor.on_key(&press_char('w')), EditorOutcome::Ignored);
}
