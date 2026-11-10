//! The text editor's tests: Vim's motions, operators, modes, and registers.

use super::*;

#[test]
fn test_the_motions_that_used_to_do_nothing_now_move() {
    let tmp = TempDir::new("motions");
    let mut editor = tmp.editor("fn one() {\n    body()\n}\n\nfn two() {\n}\n");

    // `}` to the blank line past the first run of text, `{` back again.
    press(&mut editor, "}");
    assert_eq!(editor.buffer().row(), 3, "the blank line");
    press(&mut editor, "{");
    assert_eq!(editor.buffer().row(), 0, "and back to the top");

    // `%` between a bracket and its match, lines apart.
    press(&mut editor, "$");
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (0, 9));
    press(&mut editor, "%");
    assert_eq!(
        (editor.buffer().row(), editor.buffer().col()),
        (2, 0),
        "the closing brace"
    );
    press(&mut editor, "%");
    assert_eq!(
        (editor.buffer().row(), editor.buffer().col()),
        (0, 9),
        "back"
    );

    // `ge` to the end of the word before.
    press(&mut editor, "j$");
    press(&mut editor, "ge");
    assert_eq!(
        (editor.buffer().row(), editor.buffer().col()),
        (1, 7),
        "`body`"
    );
}

#[test]
fn test_zz_puts_the_cursor_line_in_the_middle_of_the_pane() {
    let tmp = TempDir::new("scroll");
    let text: String = (0..40).map(|n| format!("line {n}\n")).collect();
    let mut editor = tmp.editor(&text);
    paint_frame(&mut editor, 11);
    press(&mut editor, "20G");
    paint_frame(&mut editor, 11);
    press(&mut editor, "zz");
    assert_eq!(editor.doc().scroll, 19 - 5, "the line sits five rows down");
    press(&mut editor, "zt");
    assert_eq!(editor.doc().scroll, 19, "and on the top row");
}

#[test]
fn test_a_new_line_keeps_the_indent_of_the_one_it_was_split_from() {
    // A line that starts back at column zero is the indentation retyped
    // every time, in code that is nothing but indented.
    let tmp = TempDir::new("indent");
    let mut editor = tmp.editor("    let x = 1;\n");
    press(&mut editor, "$a");
    editor.on_key(&key(KeyCode::Enter));
    press(&mut editor, "y");
    assert_eq!(editor.buffer().lines(), ["    let x = 1;", "    y"]);
}

#[test]
fn test_an_operator_takes_the_word_object_under_the_cursor() {
    let tmp = TempDir::new("objects");

    let mut inner = tmp.editor("alpha beta gamma\n");
    press(&mut inner, "wdiw");
    assert_eq!(inner.buffer().lines(), ["alpha  gamma"], "`diw`");

    let mut around = tmp.editor("alpha beta gamma\n");
    press(&mut around, "wdaw");
    assert_eq!(
        around.buffer().lines(),
        ["alpha gamma"],
        "`daw` the blank too"
    );
}

#[test]
fn test_the_quote_and_bracket_objects_reach_inside_what_holds_them() {
    let tmp = TempDir::new("objects-held");

    let mut quoted = tmp.editor("say \"hello there\" now\n");
    type_keys(&mut quoted, "ci\"gone<esc>");
    assert_eq!(quoted.buffer().lines(), ["say \"gone\" now"]);

    let mut nested = tmp.editor("call(one, two)\n");
    press(&mut nested, "fodi(");
    assert_eq!(nested.buffer().lines(), ["call()"], "`di(` from inside");

    let mut around = tmp.editor("call(one, two)\n");
    press(&mut around, "foda(");
    assert_eq!(around.buffer().lines(), ["call"], "`da(` takes the pair");
}

#[test]
fn test_a_paragraph_object_takes_whole_lines() {
    let tmp = TempDir::new("paragraph");
    let mut editor = tmp.editor("one\ntwo\n\nthree\n");
    press(&mut editor, "dap");
    assert_eq!(editor.buffer().lines(), ["three"], "the run and its blank");
}

#[test]
fn test_a_visual_selection_is_what_the_next_operator_acts_on() {
    let tmp = TempDir::new("visual");

    let mut chars = tmp.editor("alpha beta\n");
    press(&mut chars, "vlld");
    assert_eq!(chars.buffer().lines(), ["ha beta"], "three cells taken");
    assert_eq!(
        chars.modal.mode(),
        EditMode::Normal,
        "and the selection is over"
    );

    let mut lines = tmp.editor("one\ntwo\nthree\n");
    press(&mut lines, "Vjd");
    assert_eq!(lines.buffer().lines(), ["three"], "two whole lines");

    let mut objects = tmp.editor("alpha beta\n");
    press(&mut objects, "viwy");
    assert_eq!(
        objects.buffer().register().text,
        "alpha",
        "`viw` selects it"
    );
}

#[test]
fn test_visual_o_swaps_which_end_of_the_selection_moves() {
    let tmp = TempDir::new("visual-o");
    let mut editor = tmp.editor("alphabet\n");
    press(&mut editor, "llvll");
    assert_eq!(
        (editor.buffer().col(), editor.selection().unwrap().start.1),
        (4, 2)
    );
    press(&mut editor, "o");
    assert_eq!(editor.buffer().col(), 2, "the cursor is on the other end");
    press(&mut editor, "hd");
    assert_eq!(editor.buffer().lines(), ["abet"], "which is what moved");
}

#[test]
fn test_the_indent_operators_shift_whole_lines() {
    let tmp = TempDir::new("shift");
    let mut editor = tmp.editor("one\ntwo\n");
    press(&mut editor, ">>");
    assert_eq!(editor.buffer().lines(), ["    one", "two"]);
    press(&mut editor, "<<");
    assert_eq!(editor.buffer().lines(), ["one", "two"]);
    press(&mut editor, "Vj>");
    assert_eq!(
        editor.buffer().lines(),
        ["    one", "    two"],
        "over a selection"
    );
}

#[test]
fn test_the_line_tail_commands_change_and_delete_to_the_end() {
    let tmp = TempDir::new("tail");

    let mut deleted = tmp.editor("alpha beta\n");
    press(&mut deleted, "wD");
    assert_eq!(deleted.buffer().lines(), ["alpha "]);

    let mut changed = tmp.editor("alpha beta\n");
    type_keys(&mut changed, "wCtwo<esc>");
    assert_eq!(changed.buffer().lines(), ["alpha two"]);

    let mut swapped = tmp.editor("alpha\n");
    type_keys(&mut swapped, "sb<esc>");
    assert_eq!(swapped.buffer().lines(), ["blpha"], "`s` types over one");

    let mut whole = tmp.editor("alpha\nbeta\n");
    type_keys(&mut whole, "Sgone<esc>");
    assert_eq!(
        whole.buffer().lines(),
        ["gone", "beta"],
        "`S` the whole line"
    );
}

#[test]
fn test_replace_mode_writes_over_what_is_already_there() {
    let tmp = TempDir::new("replace");
    let mut editor = tmp.editor("alpha\n");
    type_keys(&mut editor, "Rbet<esc>");
    assert_eq!(editor.buffer().lines(), ["betha"]);
    assert_eq!(editor.modal.mode(), EditMode::Normal);
}

#[test]
fn test_ctrl_a_and_ctrl_x_move_the_number_under_the_cursor() {
    let tmp = TempDir::new("numbers");
    let mut editor = tmp.editor("port 8080\n");
    editor.on_key(&ctrl('a'));
    assert_eq!(editor.buffer().lines(), ["port 8081"]);
    press(&mut editor, "5");
    editor.on_key(&ctrl('x'));
    assert_eq!(editor.buffer().lines(), ["port 8076"], "with a count");
}

#[test]
fn test_a_char_search_moves_to_the_character_and_repeats() {
    let tmp = TempDir::new("find");
    let mut editor = tmp.editor("one, two, three\n");
    press(&mut editor, "f,");
    assert_eq!(editor.buffer().col(), 3);
    press(&mut editor, ";");
    assert_eq!(editor.buffer().col(), 8, "the next one");
    press(&mut editor, ",");
    assert_eq!(editor.buffer().col(), 3, "and back to the one before");
    press(&mut editor, "0dt,");
    assert_eq!(
        editor.buffer().lines(),
        [", two, three"],
        "`dt` stops short"
    );
}

#[test]
fn test_a_named_register_keeps_what_was_yanked_into_it() {
    let tmp = TempDir::new("registers");
    let mut editor = tmp.editor("one\ntwo\n");
    press(&mut editor, "\"ayy");
    press(&mut editor, "jdd");
    assert_eq!(editor.buffer().lines(), ["one"], "the unnamed take");
    press(&mut editor, "\"ap");
    assert_eq!(editor.buffer().lines(), ["one", "one"], "and the named one");
}

#[test]
fn test_the_dot_key_does_the_last_change_again() {
    let tmp = TempDir::new("dot");

    let mut editor = tmp.editor("aaaa\n");
    press(&mut editor, "x..");
    assert_eq!(editor.buffer().lines(), ["a"], "three characters gone");

    let mut typed = tmp.editor("one\ntwo\n");
    type_keys(&mut typed, "Ix <esc>");
    type_keys(&mut typed, "j.");
    assert_eq!(typed.buffer().lines(), ["x one", "x two"], "the typing too");

    // An undo is not a change to be done again.
    let mut undone = tmp.editor("aaaa\n");
    press(&mut undone, "xu.");
    assert_eq!(undone.buffer().lines(), ["aaa"], "the `x`, not the `u`");
}

#[test]
fn test_a_search_goes_to_the_text_and_n_carries_on() {
    let tmp = TempDir::new("search");
    let mut editor = tmp.editor("one two\nthree\ntwo again\n");
    let outcome = editor.on_key(&key(KeyCode::Char('/')));
    let EditorOutcome::Prompt(request) = outcome else {
        panic!("a search asks for what to look for");
    };
    editor.on_prompt(PromptReply {
        answer: Some("two".to_string()),
        tag: request.tag,
    });
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (0, 4));
    press(&mut editor, "n");
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (2, 0));
    press(&mut editor, "N");
    assert_eq!(
        (editor.buffer().row(), editor.buffer().col()),
        (0, 4),
        "back again"
    );
}

#[test]
fn test_the_star_key_looks_for_the_word_under_the_cursor() {
    let tmp = TempDir::new("star");
    let mut editor = tmp.editor("widget\nother\nwidget\n");
    press(&mut editor, "*");
    assert_eq!(editor.buffer().row(), 2);
}

#[test]
fn test_a_mark_is_a_place_to_come_back_to() {
    let tmp = TempDir::new("marks");
    let mut editor = tmp.editor("one\ntwo\nthree\nfour\n");
    press(&mut editor, "jjma");
    press(&mut editor, "gg");
    press(&mut editor, "'a");
    assert_eq!(editor.buffer().row(), 2, "the mark");
    press(&mut editor, "''");
    assert_eq!(editor.buffer().row(), 0, "and back where the jump started");
}

#[test]
fn test_change_list_walks_older_and_newer_edits_with_g_semicolon_and_comma() {
    let tmp = TempDir::new("changelist");
    let mut editor = tmp.editor("one\ntwo\nthree\n");
    type_keys(&mut editor, "ix<esc>");
    type_keys(&mut editor, "jjiy<esc>");
    type_keys(&mut editor, "k");
    assert_eq!(editor.buffer().row(), 1);
    type_keys(&mut editor, "g;");
    assert_eq!(editor.buffer().row(), 2);
    type_keys(&mut editor, "g;");
    assert_eq!(editor.buffer().row(), 0);
    type_keys(&mut editor, "g,");
    assert_eq!(editor.buffer().row(), 2);
}

#[test]
fn test_a_click_puts_the_cursor_under_the_pointer_and_a_drag_selects() {
    let tmp = TempDir::new("mouse");
    let mut editor = tmp.editor("one\ntwo\nthree\n");
    paint_frame(&mut editor, 6);
    editor.on_mouse(Point {
        col: 2,
        drag: false,
        row: 1,
    });
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (1, 2));
    editor.on_mouse(Point {
        col: 4,
        drag: true,
        row: 2,
    });
    assert_eq!(editor.modal.mode(), EditMode::Visual, "a drag selects");
    let selection = editor.selection().expect("a selection");
    assert_eq!((selection.start, selection.end), ((1, 2), (2, 4)));
}

#[test]
fn test_an_operator_over_a_paragraph_motion_stays_within_the_lines() {
    // `d}` is exclusive and charwise in Vim: it takes to the blank line
    // without taking the blank line itself.
    let tmp = TempDir::new("paragraph-operator");
    let mut editor = tmp.editor("one\ntwo\n\nthree\n");
    press(&mut editor, "d}");
    assert_eq!(editor.buffer().lines(), ["", "three"]);
}

#[test]
fn test_ctrl_slash_comments_the_selected_lines_and_the_cursor_line_back() {
    let tmp = TempDir::new("comment");
    let path = tmp.file("a.py", "x = 1\ny = 2\nz = 3\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    press(&mut editor, "Vj");
    press_ctrl(&mut editor, '/');
    assert_eq!(editor.buffer().lines(), ["# x = 1", "# y = 2", "z = 3"]);
    assert_eq!(
        editor.modal.mode(),
        EditMode::Normal,
        "the selection is done with"
    );

    // Back on one line, in Insert mode, the caret keeps its character.
    press(&mut editor, "A");
    press_ctrl(&mut editor, '/');
    assert_eq!(editor.buffer().lines()[1], "y = 2");
    assert_eq!(editor.buffer().col(), 5, "still at the end of the line");
}

#[test]
fn test_insert_mode_walks_and_kills_by_word_with_the_alt_chords() {
    let tmp = TempDir::new("emacs-words");
    let mut editor = tmp.editor("one two three\n");
    press(&mut editor, "A");
    press_alt(&mut editor, KeyCode::Char('b'));
    assert_eq!(editor.buffer().col(), 8, "back to the start of three");
    press_alt(&mut editor, KeyCode::Char('b'));
    assert_eq!(editor.buffer().col(), 4);
    press_alt(&mut editor, KeyCode::Char('f'));
    assert_eq!(editor.buffer().col(), 7, "forward to the end of two");
    press_alt(&mut editor, KeyCode::Char('d'));
    assert_eq!(editor.buffer().lines(), ["one two"]);
    press_alt(&mut editor, KeyCode::Backspace);
    assert_eq!(editor.buffer().lines(), ["one "]);
    // The two kills ran back to back, so they come back as one.
    press_ctrl(&mut editor, 'y');
    assert_eq!(
        editor.buffer().lines(),
        ["one two three"],
        "the whole run comes back"
    );
}

#[test]
fn test_ctrl_backspace_kills_the_word_behind_the_cursor_as_alt_backspace_does() {
    let tmp = TempDir::new("ctrl-backspace");
    let mut editor = tmp.editor("one two three\n");
    press(&mut editor, "A");
    editor.on_key(&Key {
        ctrl: true,
        ..key(KeyCode::Backspace)
    });
    assert_eq!(editor.buffer().lines(), ["one two "]);
    // Back to back with `M-DEL`, the two kills come back as one.
    press_alt(&mut editor, KeyCode::Backspace);
    assert_eq!(editor.buffer().lines(), ["one "]);
    press_ctrl(&mut editor, 'y');
    assert_eq!(editor.buffer().lines(), ["one two three"]);
}

#[test]
fn test_insert_mode_edits_around_the_cursor_with_the_ctrl_chords() {
    let tmp = TempDir::new("emacs-ctrl");
    let mut editor = tmp.editor("abcd\nxyz\n");
    press(&mut editor, "i");
    press_ctrl(&mut editor, 'f');
    press_ctrl(&mut editor, 'f');
    press_ctrl(&mut editor, 't');
    assert_eq!(editor.buffer().lines(), ["acbd", "xyz"], "transposed");
    press_ctrl(&mut editor, 'd');
    assert_eq!(editor.buffer().lines(), ["acb", "xyz"], "deleted under it");
    press_ctrl(&mut editor, 'h');
    assert_eq!(editor.buffer().lines(), ["ac", "xyz"], "deleted behind it");
    press_ctrl(&mut editor, 'n');
    assert_eq!(editor.buffer().row(), 1, "down a line");
    press_ctrl(&mut editor, 'p');
    assert_eq!(editor.buffer().row(), 0, "and back up");
    press_ctrl(&mut editor, 'o');
    assert_eq!(editor.buffer().lines(), ["ac", "", "xyz"].map(String::from));
    assert_eq!(
        editor.buffer().row(),
        0,
        "the cursor stays ahead of the break"
    );
}

#[test]
fn test_insert_mode_kills_a_line_and_yanks_it_with_ctrl_k_and_ctrl_y() {
    let tmp = TempDir::new("emacs-kill");
    let mut editor = tmp.editor("abc def\n");
    press(&mut editor, "i");
    press_alt(&mut editor, KeyCode::Char('f'));
    press_ctrl(&mut editor, 'k');
    assert_eq!(editor.buffer().lines(), ["abc"]);
    press(&mut editor, "X");
    press_ctrl(&mut editor, 'y');
    assert_eq!(editor.buffer().lines(), ["abcX def"]);
}

#[test]
fn test_the_space_bar_types_a_space_in_insert_and_after_r() {
    // Regression: the space bar is a key code of its own, which insert
    // mode did not match, so a commit message could not hold a space.
    let tmp = TempDir::new("space");
    let mut editor = tmp.editor("ab\n");
    press(&mut editor, "i");
    editor.on_key(&key(KeyCode::Space));
    press(&mut editor, "c");
    assert_eq!(editor.buffer().lines(), [" cab"]);

    editor.on_key(&key(KeyCode::Escape));
    press(&mut editor, "0lr");
    editor.on_key(&key(KeyCode::Space));
    assert_eq!(
        editor.buffer().lines(),
        ["  ab"],
        "r then space replaces the c"
    );
}

#[test]
fn test_the_space_bar_pages_in_normal_mode_as_c_f_and_c_b_do() {
    let text: String = (0..200).map(|n| format!("line {n}\n")).collect();
    for wrap in [false, true] {
        let tmp = TempDir::new(&format!("space-page-{wrap}"));
        let paged = |keys: &[Key]| {
            let mut editor = tmp.editor(&text);
            editor.content(20, 40, wrap);
            for key in keys {
                editor.on_key(key);
            }
            editor.buffer().row()
        };
        let space = key(KeyCode::Space);
        let shift_space = Key {
            shift: true,
            ..key(KeyCode::Space)
        };
        let down = paged(&[ctrl('f')]);
        assert!(down > 0, "C-f pages (wrap {wrap})");
        assert_eq!(paged(&[space]), down, "SPC is C-f (wrap {wrap})");
        assert_eq!(
            paged(&[space, space, shift_space]),
            paged(&[ctrl('f'), ctrl('f'), ctrl('b')]),
            "S-SPC is C-b (wrap {wrap})"
        );
    }
    // A space still answers a key that waits for a character.
    let tmp = TempDir::new("space-find");
    let mut editor = tmp.editor("a b\n");
    press(&mut editor, "f");
    editor.on_key(&key(KeyCode::Space));
    assert_eq!(
        (editor.buffer().row(), editor.buffer().col()),
        (0, 1),
        "f then space finds it"
    );
}

#[test]
fn test_a_line_taken_with_dd_comes_back_with_p() {
    let tmp = TempDir::new("dd");
    let mut editor = tmp.editor("one\ntwo\nthree\n");
    press(&mut editor, "dd");
    assert_eq!(editor.buffer().lines(), ["two", "three"]);
    press(&mut editor, "p");
    assert_eq!(editor.buffer().lines(), ["two", "one", "three"]);
}

#[test]
fn test_cw_changes_the_word_without_swallowing_the_space_after_it() {
    // `dw` takes the space and `cw` does not: the difference is the whole
    // reason Vim treats them separately, and getting it wrong runs the
    // changed word into the next one.
    let tmp = TempDir::new("cw");

    let mut change = tmp.editor("alpha beta\n");
    press(&mut change, "cw");
    assert_eq!(change.modal.mode(), EditMode::Insert);
    assert_eq!(change.buffer().lines(), [" beta"]);

    let mut delete = tmp.editor("alpha beta\n");
    press(&mut delete, "dw");
    assert_eq!(delete.buffer().lines(), ["beta"]);
}

#[test]
fn test_the_inclusive_motions_take_the_character_they_land_on() {
    // `d$` leaving the last character behind, and `de` leaving a word's
    // final letter, are the classic off-by-one in an operator.
    let tmp = TempDir::new("inclusive");

    let mut dollar = tmp.editor("keep this\n");
    press(&mut dollar, "ll");
    editor_key(&mut dollar, KeyCode::Char('d'));
    editor_key(&mut dollar, KeyCode::Char('$'));
    assert_eq!(dollar.buffer().lines(), ["ke"]);

    let mut end = tmp.editor("alpha beta\n");
    press(&mut end, "de");
    assert_eq!(end.buffer().lines(), [" beta"]);
}

#[test]
fn test_an_operator_over_a_motion_that_crosses_lines_takes_whole_lines() {
    // `dj` is linewise in Vim: taking only the characters between the two
    // columns would leave two half-lines spliced together.
    let tmp = TempDir::new("linewise");
    let mut editor = tmp.editor("one\ntwo\nthree\n");
    press(&mut editor, "dj");
    assert_eq!(editor.buffer().lines(), ["three"]);
}

#[test]
fn test_a_count_applies_to_the_command_that_follows_it() {
    let tmp = TempDir::new("count");
    let mut editor = tmp.editor("abcdef\n");
    press(&mut editor, "3x");
    assert_eq!(editor.buffer().lines(), ["def"]);

    let mut lines = tmp.editor("one\ntwo\nthree\nfour\n");
    press(&mut lines, "2dd");
    assert_eq!(lines.buffer().lines(), ["three", "four"]);
}

#[test]
fn test_a_leading_zero_is_the_line_start_rather_than_a_count() {
    // Typed after a digit it is part of the count; typed first it is the
    // motion to column one, and reading it as a count of zero would make
    // the next command act ten times or not at all.
    let tmp = TempDir::new("zero");
    let mut editor = tmp.editor("abcdefghijkl\n");
    press(&mut editor, "lll0x");
    assert_eq!(editor.buffer().lines(), ["bcdefghijkl"]);

    let mut ten = tmp.editor("abcdefghijkl\n");
    press(&mut ten, "10x");
    assert_eq!(ten.buffer().lines(), ["kl"]);
}

#[test]
fn test_typing_lands_in_the_text_and_escape_steps_back_onto_it() {
    let tmp = TempDir::new("insert");
    let mut editor = tmp.editor("bc\n");
    press(&mut editor, "i");
    assert_eq!(editor.modal.mode(), EditMode::Insert);
    press(&mut editor, "a");
    assert_eq!(editor.buffer().lines(), ["abc"]);

    editor.on_key(&key(KeyCode::Escape));
    assert_eq!(editor.modal.mode(), EditMode::Normal);
    assert_eq!(
        editor.buffer().col(),
        0,
        "back onto the character just typed"
    );
}

#[test]
fn test_a_lone_ctrl_c_is_forgotten_by_the_next_key() {
    let tmp = TempDir::new("chord");
    let (mut editor, path) = message_editor(&tmp);
    editor.on_key(&ctrl('c'));
    press(&mut editor, "a");
    assert_eq!(editor.on_key(&ctrl('c')), EditorOutcome::Consumed);
    assert_eq!(
        std::fs::read_to_string(path).expect("reads"),
        "\n# Please enter\n",
        "nothing was saved"
    );
}

#[test]
fn test_insert_mode_offers_only_what_does_not_type() {
    let tmp = TempDir::new("menu-insert");
    let mut editor = tmp.editor("alpha\n");
    press(&mut editor, "i");
    assert_eq!(
        menu_labels(&editor),
        ["Paste", "Quick Fix…", "Toggle Comment", "Save"]
    );
}

#[test]
fn test_ctrl_l_puts_the_line_in_the_middle_then_at_the_top_then_the_bottom() {
    let tmp = TempDir::new("recenter");
    let text: String = (0..100).map(|n| format!("line {n}\n")).collect();
    let mut editor = tmp.editor(&text);
    paint_frame(&mut editor, 20);
    type_keys(&mut editor, "50G");
    editor.on_key(&ctrl('l'));
    assert_eq!(editor.doc().scroll, 49 - 10);
    editor.on_key(&ctrl('l'));
    assert_eq!(editor.doc().scroll, 49, "at the top");
    editor.on_key(&ctrl('l'));
    assert_eq!(editor.doc().scroll, 49 - 19, "at the bottom");
}

#[test]
fn test_an_offset_is_gone_to_and_said_counting_a_line_end_as_one() {
    use winter_model::editor::{EDITOR_GOTO_OFFSET, EDITOR_SHOW_OFFSET};
    let tmp = TempDir::new("offset");
    let mut editor = tmp.editor("abc\nde\nf\n");
    let EditorOutcome::Prompt(ask) = editor.run_command(EDITOR_GOTO_OFFSET).expect("a command")
    else {
        panic!("asks for the offset");
    };
    editor.on_prompt(PromptReply {
        answer: Some("5".to_string()),
        tag: ask.tag,
    });
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (1, 1));
    assert!(
        !editor.is_flashing(),
        "a jump within the file does not flash the line"
    );
    editor.run_command(EDITOR_SHOW_OFFSET);
    assert_eq!(
        editor.take_notice().map(|notice| notice.text),
        Some("offset 5".to_string())
    );
}

#[test]
fn test_shown_where_the_cursor_is_a_span_the_cursor_is_in_keeps_its_markers() {
    let tmp = TempDir::new("conceal-cursor");
    let path = tmp.file("notes.md", "A **bold** word\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    editor.set_markup_markers(winter_model::paint::MarkupMarkers::ShowOnCursor);
    assert_eq!(
        painted_text(&editor.content(4, 60, false), 0),
        "A bold word",
        "the cursor is at the line's start"
    );
    type_keys(&mut editor, "fb");
    assert_eq!(
        painted_text(&editor.content(4, 60, false), 0),
        "A **bold** word"
    );
    editor.set_markup_markers(winter_model::paint::MarkupMarkers::Visible);
    type_keys(&mut editor, "0");
    assert_eq!(
        painted_text(&editor.content(4, 60, false), 0),
        "A **bold** word",
        "never hidden"
    );
}

#[test]
fn test_two_tabs_on_one_file_are_views_of_one_buffer() {
    let tmp = TempDir::new("one-buffer");
    let path = tmp.file("notes.txt", "one\ntwo\nthree\n");
    let mut left = TextEditor::new(path.clone(), None).expect("opens");
    let mut right = TextEditor::new(path.clone(), Some(3)).expect("joins");
    right.content(10, 40, false);
    assert_eq!(right.buffer().row(), 2, "each tab its own cursor");
    // An edit in the left is the right's, the right's cursor kept on its line.
    type_keys(&mut left, "Onew<esc>");
    right.content(10, 40, false);
    assert_eq!(right.buffer().lines(), ["new", "one", "two", "three"]);
    assert_eq!(right.buffer().row(), 3, "still on three");
    assert!(right.is_dirty(), "one buffer, one unsaved state");
    // Saved in the right, it is saved for the left, and neither takes the
    // write for a change made elsewhere.
    right.on_key(&ctrl('s'));
    assert!(!left.is_dirty() && !right.is_dirty());
    assert_eq!(
        std::fs::read_to_string(&path).expect("reads"),
        "new\none\ntwo\nthree\n"
    );
    left.on_tick(Instant::now() + Duration::from_secs(5));
    assert!(left.take_notice().is_none(), "no word of a change on disk");
    // Undone in the right, it is undone in the left.
    type_keys(&mut right, "u");
    left.content(10, 40, false);
    assert_eq!(left.buffer().lines(), ["one", "two", "three"]);
}

#[test]
fn test_ctrl_shift_k_deletes_whole_line_in_normal_mode() {
    let tmp = TempDir::new("c-s-k-normal");
    let mut editor = tmp.editor("one\ntwo\nthree\n");
    press(&mut editor, "j");
    assert_eq!(editor.buffer().row(), 1);
    editor.on_key(&ctrl_shift('k'));
    assert_eq!(editor.buffer().lines(), ["one", "three"]);
    assert_eq!(editor.buffer().row(), 1);

    // Undo restores the line.
    press(&mut editor, "u");
    assert_eq!(editor.buffer().lines(), ["one", "two", "three"]);
}

#[test]
fn test_ctrl_shift_k_deletes_whole_line_in_insert_mode_and_stays_in_insert() {
    let tmp = TempDir::new("c-s-k-insert");
    let mut editor = tmp.editor("alpha\nbeta\ngamma\n");
    press(&mut editor, "ji");
    assert_eq!(editor.modal.mode(), EditMode::Insert);
    assert_eq!(editor.buffer().row(), 1);
    editor.on_key(&ctrl_shift('k'));
    assert_eq!(editor.buffer().lines(), ["alpha", "gamma"]);
    assert_eq!(
        editor.modal.mode(),
        EditMode::Insert,
        "remains in insert mode"
    );

    // Continued typing enters text into the current line.
    press(&mut editor, "xyz");
    assert_eq!(editor.buffer().lines(), ["alpha", "xyzgamma"]);
}

#[test]
fn test_ctrl_shift_k_deletes_selection_in_visual_mode() {
    let tmp = TempDir::new("c-s-k-visual");
    let mut editor = tmp.editor("a\nb\nc\nd\n");
    press(&mut editor, "jVj");
    assert_eq!(editor.modal.mode(), EditMode::Visual);
    editor.on_key(&ctrl_shift('k'));
    assert_eq!(editor.buffer().lines(), ["a", "d"]);
    assert_eq!(
        editor.modal.mode(),
        EditMode::Normal,
        "returns to normal mode"
    );
}

#[test]
fn test_transpose_commands() {
    let tmp = TempDir::new("transpose");
    let mut editor = tmp.editor("first line\nsecond line\n");

    // Transpose lines
    editor.buffer_mut().move_to(1, 0);
    editor.run_command(winter_model::editor::TRANSPOSE_LINES);
    assert_eq!(editor.buffer().lines(), ["second line", "first line"]);

    // Transpose words
    editor.buffer_mut().move_to(0, 0);
    editor.run_command(winter_model::editor::TRANSPOSE_WORDS);
    assert_eq!(editor.buffer().lines(), ["line second", "first line"]);
}

#[test]
fn test_word_case_conversions() {
    let tmp = TempDir::new("word-case");
    let mut editor = tmp.editor("hello world\n");
    editor.buffer_mut().move_to(0, 0);

    // Uppercase
    editor.run_command(winter_model::editor::UPPERCASE_WORD);
    assert_eq!(editor.buffer().lines(), ["HELLO world"]);

    // Lowercase
    editor.buffer_mut().move_to(0, 0);
    editor.run_command(winter_model::editor::LOWERCASE_WORD);
    assert_eq!(editor.buffer().lines(), ["hello world"]);

    // Capitalize
    editor.buffer_mut().move_to(0, 0);
    editor.run_command(winter_model::editor::CAPITALIZE_WORD);
    assert_eq!(editor.buffer().lines(), ["Hello world"]);
}

#[test]
fn test_toggle_wrap_and_select_paragraph_commands() {
    let tmp = TempDir::new("paragraph-commands");
    let mut editor = tmp.editor("line one\nline two\nline three\n");
    editor.buffer_mut().move_to(1, 0);

    // Toggle wrap joins multi-line paragraph
    editor.run_command(winter_model::editor::TOGGLE_WRAP_PARAGRAPH);
    assert_eq!(editor.buffer().lines().len(), 1);
    assert_eq!(editor.buffer().lines()[0], "line one line two line three");

    // Select paragraph selects all rows
    editor.run_command(winter_model::editor::SELECT_PARAGRAPH);
    assert_eq!(editor.modal.mode(), EditMode::Visual);
}

#[test]
fn test_sexp_editor_commands() {
    let tmp = TempDir::new("sexp-commands");
    let mut editor = tmp.editor("(foo (bar)) baz\n");

    // Slurp forward
    editor.buffer_mut().move_to(0, 2); // inside (foo ...)
    editor.run_command(winter_model::editor::SEXP_SLURP_FORWARD);
    assert_eq!(editor.buffer().lines(), ["(foo (bar) baz)"]);

    // Barf forward: (foo (bar) baz) -> (foo (bar)) baz
    editor.run_command(winter_model::editor::SEXP_BARF_FORWARD);
    assert_eq!(editor.buffer().lines(), ["(foo (bar)) baz"]);

    // Splice: unwraps outer (foo (bar)) -> foo (bar)
    editor.run_command(winter_model::editor::SEXP_SPLICE);
    assert_eq!(editor.buffer().lines(), ["foo (bar) baz"]);
}

#[test]
fn test_an_edit_after_undo_keeps_what_was_undone_to_be_chosen_back() {
    let tmp = TempDir::new("undo-branches");
    let path = tmp.file("notes.txt", "one\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    type_keys(&mut editor, "A two<esc>");
    type_keys(&mut editor, "u");
    type_keys(&mut editor, "A three<esc>");
    assert_eq!(editor.buffer().lines()[0], "one three");
    let EditorOutcome::Pick(offered) = editor
        .run_command(winter_model::editor::UNDO_BRANCHES)
        .expect("an editor command")
    else {
        panic!("the branches offered");
    };
    assert_eq!(offered.items.len(), 1);
    assert!(
        offered.items[0].starts_with("1. 1 change from line 1"),
        "{:?}",
        offered.items
    );
    let reply = winter_model::editor::PromptReply {
        answer: Some(offered.items[0].clone()),
        tag: offered.tag,
    };
    editor.on_prompt(reply);
    assert_eq!(
        editor.buffer().lines()[0],
        "one two",
        "the undone typing back"
    );
}

#[test]
fn test_the_selection_grows_through_word_brackets_and_line_and_shrinks_back() {
    let tmp = TempDir::new("expand-selection");
    let path = tmp.file("main.rs", "foo(bar, baz)\nnext\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    type_keys(&mut editor, "fa");
    let grow = |editor: &mut TextEditor| {
        editor.run_command(winter_model::editor::EXPAND_SELECTION);
        let selected = editor.selection().expect("a selection");
        (selected.start, selected.end)
    };
    assert_eq!(grow(&mut editor), ((0, 4), (0, 6)), "the word");
    assert_eq!(
        grow(&mut editor),
        ((0, 4), (0, 11)),
        "what the brackets hold"
    );
    assert_eq!(grow(&mut editor), ((0, 3), (0, 12)), "and the brackets");
    assert_eq!(grow(&mut editor), ((0, 0), (0, 12)), "the line");
    assert_eq!(
        grow(&mut editor),
        ((0, 0), (1, 3)),
        "the paragraph, the file"
    );
    editor.run_command(winter_model::editor::SHRINK_SELECTION);
    let selected = editor.selection().expect("a selection");
    assert_eq!(
        (selected.start, selected.end),
        ((0, 0), (0, 12)),
        "back to the line"
    );
}

/// Answer the command line `:` opened with `line`.
fn ex(editor: &mut TextEditor, line: &str) {
    let reply = winter_model::editor::PromptReply {
        answer: Some(line.to_string()),
        tag: "ex",
    };
    editor.on_prompt(reply);
}

#[test]
fn test_a_global_command_types_keys_on_each_matching_line_and_one_undo_takes_it_back() {
    let tmp = TempDir::new("ex-global");
    let path = tmp.file("notes.txt", "a TODO\nb\nc TODO\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    let EditorOutcome::Prompt(asked) = editor.on_key(&key(KeyCode::Char(':'))) else {
        panic!("the command line");
    };
    assert_eq!(asked.initial, "");
    ex(&mut editor, "g/TODO/normal A!");
    assert_eq!(editor.buffer().lines(), ["a TODO!", "b", "c TODO!"]);
    ex(&mut editor, "v/TODO/d");
    assert_eq!(editor.buffer().lines(), ["a TODO!", "c TODO!"]);
    ex(&mut editor, "%s/TODO/done/");
    assert_eq!(editor.buffer().lines(), ["a done!", "c done!"]);
    type_keys(&mut editor, "u");
    assert_eq!(
        editor.buffer().lines(),
        ["a TODO!", "c TODO!"],
        "one step each"
    );
    type_keys(&mut editor, "uu");
    assert_eq!(editor.buffer().lines(), ["a TODO", "b", "c TODO"]);
}

#[test]
fn test_the_command_line_from_a_selection_acts_on_its_lines() {
    let tmp = TempDir::new("ex-selection");
    let path = tmp.file("notes.txt", "one\ntwo\nthree\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    type_keys(&mut editor, "jV");
    let EditorOutcome::Prompt(asked) = editor.on_key(&key(KeyCode::Char(':'))) else {
        panic!("the command line");
    };
    assert_eq!(asked.initial, "'<,'>");
    ex(&mut editor, "'<,'>normal I- ");
    assert_eq!(editor.buffer().lines(), ["one", "- two", "three"]);
}

#[test]
fn test_what_is_at_the_cursor_is_offered_the_actions_its_kind_has() {
    let tmp = TempDir::new("act-at-point");
    tmp.file("other.md", "a\nb\n");
    let path = tmp.file("notes.txt", "see other.md:2 or https://example.org/x.\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    type_keys(&mut editor, "fo");
    let EditorOutcome::Pick(offered) = editor
        .run_command(winter_model::editor::ACT_AT_POINT)
        .expect("a command")
    else {
        panic!("the actions offered");
    };
    assert_eq!(offered.items[0], "Open other.md", "{:?}", offered.items);
    assert!(offered
        .items
        .iter()
        .any(|item| item == "other: Change Every Occurrence"));
    let reply = winter_model::editor::PromptReply {
        answer: Some("Open other.md".to_string()),
        tag: offered.tag,
    };
    let EditorOutcome::OpenPath(target) = editor.on_prompt(reply) else {
        panic!("opened");
    };
    assert!(target.path.ends_with("other.md"));
    assert_eq!(target.line, Some(2));
    type_keys(&mut editor, "fx");
    let EditorOutcome::Pick(offered) = editor
        .run_command(winter_model::editor::ACT_AT_POINT)
        .expect("a command")
    else {
        panic!("the actions offered");
    };
    assert_eq!(
        offered.items[0],
        "Open https://example.org/x in the Browser"
    );
}
