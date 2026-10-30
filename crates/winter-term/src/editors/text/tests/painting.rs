//! The text editor's tests: what is painted, and keeping the cursor in view.

use super::*;

#[test]
fn test_the_wheel_moves_the_view_and_leaves_the_cursor() {
    // Reading further down must not drag the cursor along, or the place
    // being edited is lost the moment the wheel turns.
    let tmp = TempDir::new("wheel");
    let text: String = (0..40).map(|n| format!("line {n}\n")).collect();
    let mut editor = tmp.editor(&text);
    paint_frame(&mut editor, 11);
    editor.on_scroll(-3);
    paint_frame(&mut editor, 11);
    assert_eq!(editor.doc().scroll, 3, "down three");
    assert_eq!(editor.buffer().row(), 0, "the cursor stays on its line");
    editor.on_scroll(2);
    paint_frame(&mut editor, 11);
    assert_eq!(editor.doc().scroll, 1, "and back up two");
    assert_eq!(editor.buffer().row(), 0);
}

#[test]
fn test_where_the_cursor_is_goes_to_the_status_bar_not_the_text() {
    let tmp = TempDir::new("status");
    let path = tmp.file("a.txt", "one\ntwo\nthree\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    press(&mut editor, "jl\"a");
    let status = editor.status();
    assert_eq!(status.mode, Some(EditMode::Normal));
    assert_eq!(status.segments, ["2:2", "3 lines", "\"a"]);
    let first = crate::model::paint::row_text(&editor.content(10, 80, false).rows[0]);
    assert!(
        first.ends_with("one"),
        "the file's first line heads the pane: {first:?}"
    );

    press(&mut editor, "i");
    assert_eq!(editor.status().mode, Some(EditMode::Insert));
}

#[test]
fn test_a_key_brings_the_view_back_to_the_cursor() {
    let tmp = TempDir::new("wheel-key");
    let text: String = (0..40).map(|n| format!("line {n}\n")).collect();
    let mut editor = tmp.editor(&text);
    paint_frame(&mut editor, 11);
    editor.on_scroll(-20);
    paint_frame(&mut editor, 11);
    assert!(editor.doc().scroll > 0);
    press(&mut editor, "l");
    paint_frame(&mut editor, 11);
    assert_eq!(editor.doc().scroll, 0, "the cursor is in view again");
}

#[test]
fn test_painting_survives_a_pane_with_no_room_and_a_line_with_no_text() {
    // Every one of these indexes the line the cursor is on to draw the
    // cursor into it, and an empty line, a pane one column wide, and a
    // pane with no rows at all are where that goes wrong.
    let tmp = TempDir::new("paint");
    let mut editor = tmp.editor("one\n\nthree\n");
    for (rows, cols) in [(0, 0), (1, 1), (2, 6), (40, 80)] {
        editor.content(rows, cols, false);
        editor.content(rows, cols, true);
    }

    // On the empty line, in both modes, and past the end of a short one.
    press(&mut editor, "j");
    editor.content(10, 20, false);
    press(&mut editor, "i");
    editor.content(10, 20, false);
}

#[test]
fn test_source_is_painted_by_what_each_run_of_it_is() {
    let tmp = TempDir::new("syntax");
    let path = tmp.file("a.rs", "let n = 42; // why\n");
    let mut editor = TextEditor::new(path, None).expect("opens");

    assert_eq!(style_at(&mut editor, 0, "let"), scope("storage.type"));
    assert_eq!(style_at(&mut editor, 0, "n "), scope("variable.declaration"), "a variable where it is bound");
    assert_eq!(style_at(&mut editor, 0, " = "), Style::Normal);
    assert_eq!(style_at(&mut editor, 0, "42"), scope("constant.numeric"));
    assert_eq!(style_at(&mut editor, 0, "why"), scope("comment"));
}

#[test]
fn test_git_s_own_files_are_painted_by_git_s_grammars() {
    let tmp = TempDir::new("git-colors");
    let plan = tmp.file("git-rebase-todo", "pick abc1 subject\n");
    let mut editor = TextEditor::new(plan, None).expect("opens");
    assert_eq!(style_at(&mut editor, 0, "pick"), scope("keyword.operator"));

    let summary = format!("feat: x{}\n\n# a comment\n", "y".repeat(80));
    let message = tmp.file("COMMIT_EDITMSG", &summary);
    let mut editor = TextEditor::new(message, None).expect("opens");
    assert_eq!(
        color_of(style_at(&mut editor, 0, "feat")),
        Some("keyword"),
        "keyword-colored, and as heavy as the summary around it"
    );
    assert_eq!(
        style_at(&mut editor, 0, "yyyy"),
        scope("markup.heading.subject.git.commit")
    );
    editor.content(20, 60, false);
    let summary = editor.colors_of(0).to_vec();
    assert_eq!(
        (
            summary[commit::SUMMARY_WIDTH - 1],
            summary[commit::SUMMARY_WIDTH]
        ),
        (scope("markup.heading.subject.git.commit"), Style::Removed),
        "past the width a summary reads well at"
    );
    assert_eq!(style_at(&mut editor, 2, "a comment"), scope("comment"));

    let verbose = tmp.file(
        "MERGE_MSG",
        "x\n# ------------------------ >8 ------------------------\n\
         diff --git a/f b/f\n+added\n",
    );
    let mut editor = TextEditor::new(verbose, None).expect("opens");
    assert_eq!(
        style_at(&mut editor, 3, "added"),
        scope("markup.inserted.diff"),
        "the diff below the scissors"
    );

    let notes = tmp.file("notes.md", "```\n# code\n```\n# Title\n");
    let mut editor = TextEditor::new(notes, None).expect("opens");
    assert_eq!(
        style_at(&mut editor, 1, "code"),
        scope("markup.raw.code-fence"),
        "inside the fence"
    );
    assert_eq!(style_at(&mut editor, 3, "Title"), scope("markup.heading.1"));
}

#[test]
fn test_show_scopes_says_what_the_grammar_makes_of_the_cursor_s_character() {
    let tmp = TempDir::new("scopes");
    let mut editor = TextEditor::new(tmp.file("a.rs", "let x = 1;\n"), None).expect("opens");
    assert!(editor.token_inspect().is_none());

    let outcome = editor.run_command(SHOW_SCOPES);
    assert!(matches!(outcome, Some(EditorOutcome::Consumed)));
    let data = editor.token_inspect().expect("inspect data active");
    assert_eq!(data.character, "'l' (U+006C)");
    assert_eq!(data.position, "ln 1, col 1");
    assert_eq!(data.language, "Rust");
    assert!(data.scopes.contains(&"storage.type.rust".to_string()));
    assert!(editor.run_command("no_such_command").is_none());

    // Moving cursor updates inspect data automatically
    editor.on_key(&key(KeyCode::Char('l')));
    let data2 = editor.token_inspect().expect("still inspecting after move");
    assert_eq!(data2.character, "'e' (U+0065)");
    assert_eq!(data2.position, "ln 1, col 2");

    // Pressing Escape dismisses the inspector
    let esc_outcome = editor.on_key(&key(KeyCode::Escape));
    assert_eq!(esc_outcome, EditorOutcome::Consumed);
    assert!(editor.token_inspect().is_none(), "inspector dismissed by Escape");
}

#[test]
fn test_the_window_puts_the_inspector_away_without_leaving_the_mode() {
    // It sent an Escape, which also ended the selection being made.
    let tmp = TempDir::new("hide-scopes");
    let mut editor = TextEditor::new(tmp.file("a.rs", "let x = 1;\n"), None).expect("opens");
    editor.run_command(SHOW_SCOPES);
    editor.on_key(&key(KeyCode::Char('v')));
    let mode = editor.modal.mode();
    assert_eq!(editor.run_command(crate::model::editor::HIDE_SCOPES), Some(EditorOutcome::Consumed));
    assert!(editor.token_inspect().is_none());
    assert_eq!(editor.modal.mode(), mode, "still selecting");
    editor.run_command(crate::model::editor::HIDE_SCOPES);
    assert!(editor.token_inspect().is_none(), "hiding twice shows nothing");
}

#[test]
fn test_a_script_without_an_extension_is_known_by_its_shebang() {
    let tmp = TempDir::new("shebang");
    let path = tmp.file("deploy", "#!/usr/bin/env python3\nimport os\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    assert_eq!(style_at(&mut editor, 1, "import"), scope("keyword.control"));
}

#[test]
fn test_coloring_cut_short_by_its_budget_picks_up_where_it_stopped() {
    // A long file is colored over a few ticks: each stretch must pick up
    // from the last line worked out and come to what one long walk would.
    let tmp = TempDir::new("catch-up");
    let mut text = String::from("/* opened here\n");
    for index in 0..30 {
        text.push_str(&format!("still inside {index}\n"));
    }
    text.push_str("*/ let x = 1;\n");
    let path = tmp.file("a.rs", &text);

    let mut whole = TextEditor::new(path.clone(), None).expect("opens");
    whole.doc_mut().color_upto = 40;
    whole.doc_mut().learn_states(Duration::MAX);

    let mut stepped = TextEditor::new(path, None).expect("opens");
    stepped.doc_mut().color_upto = 40;
    stepped.doc_mut().learn_states(Duration::ZERO);
    assert!(stepped.doc().learning(), "one line per stretch");
    assert!(stepped.next_tick().is_some(), "so it asks to be ticked");
    for _ in 0..40 {
        stepped.doc_mut().learn_states(Duration::ZERO);
    }
    assert!(!stepped.doc().learning());
    assert_eq!(
        stepped.next_tick(),
        Some(stepped.disk_check_at),
        "only the look at the disk is left"
    );
    assert_eq!(stepped.doc().states, whole.doc().states);
}

#[test]
fn test_a_file_in_no_known_language_is_painted_as_the_text_it_is() {
    // Guessing at a language for a plain file colors words that mean
    // nothing, which is worse than coloring none of them.
    let tmp = TempDir::new("plain");
    let path = tmp.0.join("notes.txt");
    std::fs::write(
        &path,
        "let me note 42 things
",
    )
    .expect("temp file");
    let mut editor = TextEditor::new(path, None).expect("opens");

    assert_eq!(
        painted(&mut editor, 0),
        vec![(Style::Normal, "let me note 42 things".to_string())]
    );
}

#[test]
fn test_a_comment_opened_above_the_window_colors_what_it_covers() {
    // A line is colored by what precedes it, and what precedes it may be
    // off screen: scrolling into the middle of a block comment must not
    // show it as code.
    let tmp = TempDir::new("block");
    let path = tmp.0.join("a.rs");
    let mut text = String::from(
        "/* opened here
",
    );
    for index in 0..40 {
        text.push_str(&format!(
            "still inside {index}
"
        ));
    }
    text.push_str(
        "*/ let x = 1;
",
    );
    std::fs::write(&path, text).expect("temp file");
    let mut editor = TextEditor::new(path, None).expect("opens");

    press(&mut editor, "G");
    editor.content(20, 60, false);
    let last = editor.buffer().lines().len() - 1;
    let visible = last - editor.doc().scroll;
    let styles: Vec<Style> = painted(&mut editor, visible - 1)
        .into_iter()
        .map(|(style, _)| style)
        .collect();
    assert!(
        styles.iter().all(|style| *style == scope("comment")),
        "the line above the closer is still inside the comment: {styles:?}"
    );
}

#[test]
fn test_opening_a_comment_recolors_the_lines_under_it_as_it_is_typed() {
    // What a line means for the ones below it changes as it is edited, so
    // what was worked out about them cannot be kept.
    let tmp = TempDir::new("recolor");
    let path = tmp.0.join("a.rs");
    std::fs::write(
        &path,
        "let x = 1;
let y = 2;
",
    )
    .expect("temp file");
    let mut editor = TextEditor::new(path, None).expect("opens");
    assert_eq!(painted(&mut editor, 1)[0].0, scope("storage.type"));

    // Open a comment on the first line; the second is now inside it.
    press(&mut editor, "I");
    press(&mut editor, "/*");
    assert!(
        painted(&mut editor, 1)
            .iter()
            .all(|(style, _)| *style == scope("comment")),
        "the line below was left as code"
    );
}

#[test]
fn test_the_caret_lands_on_the_text_rather_than_in_the_line_numbers() {
    // The host puts an editor's cursor on the row's first painted character
    // unless the editor says otherwise, and here that is the line number.
    let tmp = TempDir::new("caret");
    let mut editor = tmp.editor("fn main() {\n\tlet x = 1;\n");
    let gutter = NUMBER_WIDTH + NUMBER_GAP.len();

    editor.content(10, 40, false);
    let caret = editor.caret().expect("a caret");
    assert_eq!(caret.col, gutter, "column one of the text, not of the row");
    assert!(!caret.insert);

    // A tab is one character in the file and several columns on screen,
    // so a caret that counted characters would drift left of the text.
    press(&mut editor, "jll");
    editor.content(10, 40, false);
    assert_eq!(
        editor.caret().expect("a caret").col,
        gutter + TAB_WIDTH + 1,
        "past the tab's width, then one character"
    );

    // Typing says so, so the caret can take the shape typing has.
    press(&mut editor, "i");
    assert!(editor.caret().expect("a caret").insert);
}

#[test]
fn test_a_line_wider_than_the_pane_scrolls_to_keep_the_cursor_in_view() {
    // Without this, editing past the pane's width types into a column
    // nobody can see.
    let tmp = TempDir::new("hscroll");
    let mut editor = tmp.editor(&format!("{}\n", "x".repeat(200)));
    editor.content(10, 40, false);
    assert_eq!(editor.doc().hscroll, 0);

    press(&mut editor, "$");
    editor.content(10, 40, false);
    assert!(
        editor.doc().hscroll > 0,
        "the end of the line was scrolled to"
    );

    press(&mut editor, "0");
    editor.content(10, 40, false);
    assert_eq!(editor.doc().hscroll, 0, "and back again");
}

#[test]
fn test_the_caret_counts_the_cells_wide_and_combining_characters_take() {
    // Regression: every character but a tab counted as one cell, so past
    // a CJK character the caret sat a cell left of the text.
    let tmp = TempDir::new("caret-wide");
    let mut editor = tmp.editor("\u{4e2d}e\u{301}x\n");
    let gutter = NUMBER_WIDTH + NUMBER_GAP.len();
    press(&mut editor, "ll");
    editor.content(10, 40, false);
    assert_eq!(
        editor.caret().expect("a caret").col,
        gutter + 3,
        "two cells for the wide one, one for the accented e"
    );
    assert_eq!(editor.char_at_display(0, 1), 0, "either half of it is it");
    assert_eq!(editor.char_at_display(0, 2), 1);
}

#[test]
fn test_leaving_the_end_of_a_long_line_scrolls_the_view_back_to_the_start() {
    // Regression: unwrapped, the view stayed scrolled as far right as the
    // end of a long paragraph had taken it, and every shorter line under it
    // showed as a blank gap; coming back near the start only scrolled back
    // as far as the cursor, cutting the start of each line off.
    let tmp = TempDir::new("hscroll-back");
    let text = format!("\\section{{A}}\n{}\nshort\n", "word ".repeat(80));
    let mut editor = TextEditor::new(tmp.file("paper.tex", &text), None).expect("opens");
    editor.on_key(&key(KeyCode::Char('j')));
    editor.on_key(&key(KeyCode::Char('$')));
    editor.content(10, 60, false);
    assert!(editor.doc().hscroll > 0, "the long line's end is scrolled to");
    editor.on_key(&key(KeyCode::Char('j')));
    editor.content(10, 60, false);
    assert_eq!(editor.doc().hscroll, 0, "a short line shows from its start");
    editor.on_key(&key(KeyCode::Char('k')));
    editor.on_key(&key(KeyCode::Char('0')));
    editor.on_key(&key(KeyCode::Char('w')));
    editor.content(10, 60, false);
    assert_eq!(editor.doc().hscroll, 0, "and so does a long one near its start");
}

#[test]
fn test_copying_as_rich_text_hands_over_the_painted_lines_and_their_text() {
    let tmp = TempDir::new("copy-rich");
    let path = tmp.file("a.rs", "fn main() {}\nlet x = 1;\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    press(&mut editor, "V");
    let Some(EditorOutcome::CopyStyled { rows, text }) = editor.run_command(COPY_RICH) else {
        panic!("styled rows to copy");
    };
    assert_eq!(text, "fn main() {}", "the selected line only");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].iter().any(|span| matches!(span.style, Style::Syntax(_))));
    assert_eq!(row_text(&rows[0]), "fn main() {}");
}

#[test]
fn test_a_long_line_is_folded_into_rows_that_fit_when_wrapping_and_cut_when_not() {
    let tmp = TempDir::new("wrap-whole");
    let long = "word ".repeat(40);
    let mut editor = tmp.editor(&format!("{long}\n"));
    let wrapped = editor.content(20, 40, true);
    let rows: Vec<String> = wrapped.rows.iter().map(row_text).collect();
    assert!(rows.len() > 1, "folded into screen rows");
    assert!(rows.iter().all(|row| row.chars().count() <= 40), "{rows:?}");
    assert!(rows[0].starts_with("   1 word"));
    assert!(rows[1].starts_with("     word"), "a blank gutter under the number");
    let joined: String = rows.iter().map(|row| row[5..].to_string()).collect();
    assert_eq!(joined.trim_end(), long.trim_end(), "every word on one row or another");
    let cut = editor.content(20, 40, false);
    assert!(row_text(&cut.rows[0]).chars().count() <= 40);
}

#[test]
fn test_j_and_k_go_by_screen_rows_keeping_their_column_and_gj_by_lines() {
    let tmp = TempDir::new("wrap-jk");
    let mut editor = folded(&tmp);
    type_keys(&mut editor, "wl");
    assert_eq!(editor.buffer().col(), 6);
    type_keys(&mut editor, "j");
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (0, 26), "the next screen row, same column");
    let frame = editor.content(10, 25, true);
    assert_eq!(frame.cursor_line, Some(1), "the band is on the screen row the cursor is on");
    assert_eq!(editor.caret().map(|caret| caret.col), Some(5 + 6));
    type_keys(&mut editor, "j");
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (0, 43), "the last row is short: its end");
    type_keys(&mut editor, "j");
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (1, 4), "on to the next line, the column kept");
    type_keys(&mut editor, "2k");
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (0, 26), "a count of screen rows");
    type_keys(&mut editor, "gj");
    assert_eq!(editor.buffer().row(), 1, "gj goes by whole lines");
}

#[test]
fn test_the_g_motions_go_to_the_screen_row_s_ends_and_an_operator_takes_whole_lines() {
    let tmp = TempDir::new("wrap-g");
    let mut editor = folded(&tmp);
    type_keys(&mut editor, "jll");
    type_keys(&mut editor, "g0");
    assert_eq!(editor.buffer().col(), 20);
    type_keys(&mut editor, "g$");
    assert_eq!(editor.buffer().col(), 39, "the space the row folds at");
    type_keys(&mut editor, "0");
    assert_eq!(editor.buffer().col(), 0, "0 is still the line's start");
    type_keys(&mut editor, "dj");
    assert_eq!(editor.buffer().lines(), [""], "dj takes both whole lines");
}

#[test]
fn test_typing_moves_by_screen_rows_and_to_the_row_s_ends_as_visual_line_mode_does() {
    let tmp = TempDir::new("wrap-insert");
    let mut editor = folded(&tmp);
    type_keys(&mut editor, "wi");
    editor.on_key(&ctrl('n'));
    assert_eq!(editor.buffer().col(), 25);
    editor.on_key(&ctrl('e'));
    assert_eq!(editor.buffer().col(), 39, "the row's end, before the fold");
    editor.on_key(&ctrl('a'));
    assert_eq!(editor.buffer().col(), 20, "the row's start, not the line's");
    editor.on_key(&key(KeyCode::Down));
    editor.on_key(&ctrl('e'));
    assert_eq!(editor.buffer().col(), 44, "past the line's last character on its last row");
    editor.on_key(&ctrl('p'));
    editor.on_key(&ctrl('p'));
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (0, 4), "four cells into each row up");
}

#[test]
fn test_h_and_l_land_on_the_top_and_bottom_screen_rows_painted() {
    let tmp = TempDir::new("wrap-hl");
    let mut editor = folded(&tmp);
    type_keys(&mut editor, "L");
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (1, 0), "the last row shown");
    type_keys(&mut editor, "2H");
    assert_eq!((editor.buffer().row(), editor.buffer().col()), (0, 20), "the second screen row");
}

#[test]
fn test_a_wrapped_list_item_continues_under_its_text_and_the_caret_and_clicks_follow() {
    use crate::model::paint::WrapIndent;
    let tmp = TempDir::new("wrap-indent");
    let path = tmp.file("notes.md", "- aaaa bbbb cccc dddd eeee ffff gggg hhhh\n");
    let mut editor = TextEditor::new(path, None).expect("opens");
    // 25 columns: 5 of gutter, 20 of text; the item's text starts 2 in.
    let rows: Vec<String> = editor.content(10, 25, true).rows.iter().map(row_text).collect();
    assert_eq!(rows[0], "   1 - aaaa bbbb cccc ");
    assert_eq!(rows[1], "       dddd eeee ffff ", "under the item's text");
    type_keys(&mut editor, "jl");
    editor.content(10, 25, true);
    // From the bullet, `j` lands on the indented row's first character
    // (column 17, `dddd`), and `l` steps on.
    assert_eq!(editor.buffer().col(), 18);
    assert_eq!(editor.caret().map(|caret| caret.col), Some(5 + 2 + 1), "the caret past the indent");
    type_keys(&mut editor, "k");
    assert_eq!(editor.buffer().col(), 3, "back up, the screen column kept");
    editor.on_mouse(Point {
        col: 5 + 2 + 5,
        drag: false,
        row: 1,
    });
    assert_eq!(editor.buffer().col(), 22, "a click on the indented row lands on the text under it");
    editor.set_wrap_indent(WrapIndent::None);
    let rows: Vec<String> = editor.content(10, 25, true).rows.iter().map(row_text).collect();
    assert!(rows[1].starts_with("     dddd"), "no indent: {rows:?}");
    editor.set_wrap_indent(WrapIndent::Same);
    let rows: Vec<String> = editor.content(10, 25, true).rows.iter().map(row_text).collect();
    assert!(rows[1].starts_with("     dddd"), "the line has no indentation of its own: {rows:?}");
}

#[test]
fn test_a_paging_chord_with_shift_is_the_window_s_over_wrapped_lines_too() {
    // Regression: over wrapped lines `C-S-f` paged down like `C-f`, and
    // never reached the window's file browser.
    let tmp = TempDir::new("wrap-shifted-paging");
    let text: String = (0..200).map(|n| format!("line {n}\n")).collect();
    let mut editor = tmp.editor(&text);
    editor.content(20, 40, true);
    for c in ['f', 'b', 'd', 'u'] {
        let shifted = Key {
            ctrl: true,
            shift: true,
            ..key(KeyCode::Char(c))
        };
        assert_eq!(editor.on_key(&shifted.canonical()), EditorOutcome::Ignored, "C-S-{c}");
        assert_eq!(editor.buffer().row(), 0, "C-S-{c} moves nothing");
    }
    editor.on_key(&ctrl('f'));
    assert_eq!(editor.buffer().row(), 20, "C-f still pages");
}

#[test]
fn test_over_wrapped_lines_the_view_scrolls_and_places_by_screen_rows() {
    // Regression: the view's top was a whole line, so a `j` past the
    // bottom scrolled by every row of the top line at once, and `zt` and
    // `C-l` counted lines where the pane counts screen rows.
    let tmp = TempDir::new("wrap-screen-rows");
    // Each line folds to three rows of 20 cells in a pane 25 wide:
    // "aaaa bbbb cccc dddd " / "eeee ffff gggg hhhh " / "iiii".
    let text: String = (0..10).map(|_| "aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii\n").collect();
    let mut editor = tmp.editor(&text);
    let top = |editor: &mut TextEditor, wrap: bool| painted_text(&editor.content(5, 25, wrap), 0);
    top(&mut editor, true);
    for _ in 0..5 {
        press(&mut editor, "j");
        top(&mut editor, true);
    }
    assert_eq!((editor.buffer().row(), editor.view_top()), (1, (0, 1)), "one screen row down");
    assert!(top(&mut editor, true).contains("eeee"), "the top line's second row heads the pane");
    press(&mut editor, "zt");
    assert!(top(&mut editor, true).contains("iiii"), "the cursor's screen row to the top");
    press(&mut editor, "zb");
    top(&mut editor, true);
    assert_eq!(editor.view_top(), (0, 1), "and to the bottom, five screen rows down");
    editor.on_key(&ctrl('l'));
    top(&mut editor, true);
    assert_eq!(editor.view_top(), (1, 0), "C-l: two screen rows above it");
    // Wrapping off, every line is one row and nothing is skipped.
    top(&mut editor, false);
    assert_eq!(editor.view_top(), (1, 0), "the cursor's line on top, none of it skipped");
}

#[test]
fn test_the_view_keeps_the_scroll_margin_round_the_cursor() {
    let tmp = TempDir::new("scroll-margin");
    let text: String = (0..50).map(|n| format!("line {n}\n")).collect();
    let mut editor = tmp.editor(&text);
    editor.set_scroll_margin(3);
    paint_frame(&mut editor, 10);
    press(&mut editor, "6j");
    paint_frame(&mut editor, 10);
    assert_eq!(editor.view_top(), (0, 0), "line 6 is the last before the low margin");
    press(&mut editor, "j");
    paint_frame(&mut editor, 10);
    assert_eq!(editor.view_top(), (1, 0), "line 7 scrolls the view a row");
    press(&mut editor, "zt");
    paint_frame(&mut editor, 10);
    assert_eq!(editor.view_top(), (4, 0), "zt leaves the margin above");
    press(&mut editor, "H");
    assert_eq!(editor.buffer().row(), 7, "H stops at the margin, scrolling nothing");
    press(&mut editor, "L");
    assert_eq!(editor.buffer().row(), 10);
}

#[test]
fn test_scrolling_to_the_end_by_wheel_or_keys_puts_the_last_line_on_the_bottom_row() {
    // However the end is reached, the last line sits on the pane's bottom
    // row and the pane is full: with lines that wrap, and with a last line
    // taller than the pane, which the keys once left half below the pane
    // while the frame held more rows than the pane had.
    let tmp = TempDir::new("last-line-bottom");
    let long = "word ".repeat(30);
    let tall = "word ".repeat(200);
    let files = [
        ("plain", false, (1..=100).map(|n| format!("line {n} end{n}\n")).collect::<String>()),
        ("wrapped", true, (1..=100).map(|n| format!("line {n} {long}end{n}\n")).collect::<String>()),
        ("tall", true, (1..=100).map(|n| match n == 100 {
            true => format!("line {n} {tall}end{n}\n"),
            false => format!("line {n} {long}end{n}\n"),
        }).collect::<String>()),
    ];
    for (name, wrap, body) in files {
        let path = tmp.file(&format!("{name}.txt"), &body);
        let paint = |editor: &mut TextEditor| -> Vec<String> {
            editor.content(20, 40, wrap).rows.iter().map(row_text).collect()
        };
        let reaches = |what: &str, steps: usize, mut step: Box<dyn FnMut(&mut TextEditor)>| {
            let mut editor = TextEditor::new(path.clone(), None).expect("opens");
            paint(&mut editor);
            for _ in 0..steps {
                step(&mut editor);
                let rows = paint(&mut editor);
                assert!(rows.len() <= 20, "{name}, {what}: {} rows in a 20-row pane", rows.len());
            }
            let rows = paint(&mut editor);
            assert_eq!(rows.len(), 20, "{name}, {what}: a full pane");
            assert!(rows.last().is_some_and(|row| row.contains("end100")), "{name}, {what}: {rows:?}");
        };
        reaches("the wheel", 250, Box::new(|editor| {
            editor.on_scroll(-3);
        }));
        reaches("j", 600, Box::new(|editor| press(editor, "j")));
        reaches("C-f", 40, Box::new(|editor| {
            press_ctrl(editor, 'f');
        }));
        reaches("C-d", 80, Box::new(|editor| {
            press_ctrl(editor, 'd');
        }));
    }
}
