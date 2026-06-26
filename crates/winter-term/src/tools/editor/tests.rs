//! The editor's tests.

use super::*;

use crate::model::input::KeyCode;
use crate::model::paint::SyntaxStyle;

/// A temporary directory that removes itself on drop, so a test that
/// writes files never leaks state into the next run.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("winter-editor-page-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }

    /// A file in the directory, for opening as another buffer.
    fn file(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, text).expect("temp file");
        path
    }

    /// A tool over a file holding `text`, opened at its first line.
    fn tool(&self, text: &str) -> EditorTool {
        let path = self.0.join("a.txt");
        std::fs::write(&path, text).expect("temp file");
        EditorTool::new(path, None).expect("opens")
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(code: KeyCode) -> Key {
    Key {
        alt: false,
        code,
        ctrl: false,
        shift: false,
        sup: false,
    }
}

fn ctrl(c: char) -> Key {
    Key {
        alt: false,
        code: KeyCode::Char(c),
        ctrl: true,
        shift: false,
        sup: false,
    }
}

/// Type `keys` as bare characters, the way a user would.
fn press(tool: &mut EditorTool, keys: &str) {
    for c in keys.chars() {
        tool.on_key(&key(KeyCode::Char(c)));
    }
}

/// The keys of a command, with `<esc>` for the one key a test cannot
/// spell as a character.
fn type_keys(tool: &mut EditorTool, keys: &str) {
    let mut rest = keys;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix("<esc>") {
            tool.on_key(&key(KeyCode::Escape));
            rest = tail;
            continue;
        }
        let c = rest.chars().next().expect("a key");
        tool.on_key(&key(KeyCode::Char(c)));
        rest = &rest[c.len_utf8()..];
    }
}

/// A tool that has been painted once, so what it painted is known: a
/// click, a paging key, and `zz` all measure against the pane.
fn paint_frame(tool: &mut EditorTool, rows: usize) {
    tool.content(rows, 40, false);
}

#[test]
fn test_the_motions_that_used_to_do_nothing_now_move() {
    let tmp = TempDir::new("motions");
    let mut tool = tmp.tool("fn one() {\n    body()\n}\n\nfn two() {\n}\n");

    // `}` to the blank line past the first run of text, `{` back again.
    press(&mut tool, "}");
    assert_eq!(tool.buffer().row(), 3, "the blank line");
    press(&mut tool, "{");
    assert_eq!(tool.buffer().row(), 0, "and back to the top");

    // `%` between a bracket and its match, lines apart.
    press(&mut tool, "$");
    assert_eq!((tool.buffer().row(), tool.buffer().col()), (0, 9));
    press(&mut tool, "%");
    assert_eq!(
        (tool.buffer().row(), tool.buffer().col()),
        (2, 0),
        "the closing brace"
    );
    press(&mut tool, "%");
    assert_eq!((tool.buffer().row(), tool.buffer().col()), (0, 9), "back");

    // `ge` to the end of the word before.
    press(&mut tool, "j$");
    press(&mut tool, "ge");
    assert_eq!((tool.buffer().row(), tool.buffer().col()), (1, 7), "`body`");
}

#[test]
fn test_zz_puts_the_cursor_line_in_the_middle_of_the_pane() {
    let tmp = TempDir::new("scroll");
    let text: String = (0..40).map(|n| format!("line {n}\n")).collect();
    let mut tool = tmp.tool(&text);
    paint_frame(&mut tool, 11);
    press(&mut tool, "20G");
    paint_frame(&mut tool, 11);
    press(&mut tool, "zz");
    assert_eq!(tool.doc().scroll, 19 - 5, "the line sits five rows down");
    press(&mut tool, "zt");
    assert_eq!(tool.doc().scroll, 19, "and on the top row");
}

#[test]
fn test_a_new_line_keeps_the_indent_of_the_one_it_was_split_from() {
    // A line that starts back at column zero is the indentation retyped
    // every time, in code that is nothing but indented.
    let tmp = TempDir::new("indent");
    let mut tool = tmp.tool("    let x = 1;\n");
    press(&mut tool, "$a");
    tool.on_key(&key(KeyCode::Enter));
    press(&mut tool, "y");
    assert_eq!(tool.buffer().lines(), ["    let x = 1;", "    y"]);
}

#[test]
fn test_an_operator_takes_the_word_object_under_the_cursor() {
    let tmp = TempDir::new("objects");

    let mut inner = tmp.tool("alpha beta gamma\n");
    press(&mut inner, "wdiw");
    assert_eq!(inner.buffer().lines(), ["alpha  gamma"], "`diw`");

    let mut around = tmp.tool("alpha beta gamma\n");
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

    let mut quoted = tmp.tool("say \"hello there\" now\n");
    type_keys(&mut quoted, "ci\"gone<esc>");
    assert_eq!(quoted.buffer().lines(), ["say \"gone\" now"]);

    let mut nested = tmp.tool("call(one, two)\n");
    press(&mut nested, "fodi(");
    assert_eq!(nested.buffer().lines(), ["call()"], "`di(` from inside");

    let mut around = tmp.tool("call(one, two)\n");
    press(&mut around, "foda(");
    assert_eq!(around.buffer().lines(), ["call"], "`da(` takes the pair");
}

#[test]
fn test_a_paragraph_object_takes_whole_lines() {
    let tmp = TempDir::new("paragraph");
    let mut tool = tmp.tool("one\ntwo\n\nthree\n");
    press(&mut tool, "dap");
    assert_eq!(tool.buffer().lines(), ["three"], "the run and its blank");
}

#[test]
fn test_a_visual_selection_is_what_the_next_operator_acts_on() {
    let tmp = TempDir::new("visual");

    let mut chars = tmp.tool("alpha beta\n");
    press(&mut chars, "vlld");
    assert_eq!(chars.buffer().lines(), ["ha beta"], "three cells taken");
    assert_eq!(
        chars.modal.mode(),
        EditMode::Normal,
        "and the selection is over"
    );

    let mut lines = tmp.tool("one\ntwo\nthree\n");
    press(&mut lines, "Vjd");
    assert_eq!(lines.buffer().lines(), ["three"], "two whole lines");

    let mut objects = tmp.tool("alpha beta\n");
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
    let mut tool = tmp.tool("alphabet\n");
    press(&mut tool, "llvll");
    assert_eq!(
        (tool.buffer().col(), tool.selection().unwrap().start.1),
        (4, 2)
    );
    press(&mut tool, "o");
    assert_eq!(tool.buffer().col(), 2, "the cursor is on the other end");
    press(&mut tool, "hd");
    assert_eq!(tool.buffer().lines(), ["abet"], "which is what moved");
}

#[test]
fn test_the_indent_operators_shift_whole_lines() {
    let tmp = TempDir::new("shift");
    let mut tool = tmp.tool("one\ntwo\n");
    press(&mut tool, ">>");
    assert_eq!(tool.buffer().lines(), ["    one", "two"]);
    press(&mut tool, "<<");
    assert_eq!(tool.buffer().lines(), ["one", "two"]);
    press(&mut tool, "Vj>");
    assert_eq!(
        tool.buffer().lines(),
        ["    one", "    two"],
        "over a selection"
    );
}

#[test]
fn test_the_line_tail_commands_change_and_delete_to_the_end() {
    let tmp = TempDir::new("tail");

    let mut deleted = tmp.tool("alpha beta\n");
    press(&mut deleted, "wD");
    assert_eq!(deleted.buffer().lines(), ["alpha "]);

    let mut changed = tmp.tool("alpha beta\n");
    type_keys(&mut changed, "wCtwo<esc>");
    assert_eq!(changed.buffer().lines(), ["alpha two"]);

    let mut swapped = tmp.tool("alpha\n");
    type_keys(&mut swapped, "sb<esc>");
    assert_eq!(swapped.buffer().lines(), ["blpha"], "`s` types over one");

    let mut whole = tmp.tool("alpha\nbeta\n");
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
    let mut tool = tmp.tool("alpha\n");
    type_keys(&mut tool, "Rbet<esc>");
    assert_eq!(tool.buffer().lines(), ["betha"]);
    assert_eq!(tool.modal.mode(), EditMode::Normal);
}

#[test]
fn test_ctrl_a_and_ctrl_x_move_the_number_under_the_cursor() {
    let tmp = TempDir::new("numbers");
    let mut tool = tmp.tool("port 8080\n");
    tool.on_key(&ctrl('a'));
    assert_eq!(tool.buffer().lines(), ["port 8081"]);
    press(&mut tool, "5");
    tool.on_key(&ctrl('x'));
    assert_eq!(tool.buffer().lines(), ["port 8076"], "with a count");
}

#[test]
fn test_a_char_search_moves_to_the_character_and_repeats() {
    let tmp = TempDir::new("find");
    let mut tool = tmp.tool("one, two, three\n");
    press(&mut tool, "f,");
    assert_eq!(tool.buffer().col(), 3);
    press(&mut tool, ";");
    assert_eq!(tool.buffer().col(), 8, "the next one");
    press(&mut tool, ",");
    assert_eq!(tool.buffer().col(), 3, "and back to the one before");
    press(&mut tool, "0dt,");
    assert_eq!(tool.buffer().lines(), [", two, three"], "`dt` stops short");
}

#[test]
fn test_a_named_register_keeps_what_was_yanked_into_it() {
    let tmp = TempDir::new("registers");
    let mut tool = tmp.tool("one\ntwo\n");
    press(&mut tool, "\"ayy");
    press(&mut tool, "jdd");
    assert_eq!(tool.buffer().lines(), ["one"], "the unnamed take");
    press(&mut tool, "\"ap");
    assert_eq!(tool.buffer().lines(), ["one", "one"], "and the named one");
}

#[test]
fn test_the_clipboard_register_leaves_the_tool() {
    let tmp = TempDir::new("clipboard");
    let mut tool = tmp.tool("one\ntwo\n");
    tool.on_key(&key(KeyCode::Char('"')));
    tool.on_key(&key(KeyCode::Char('+')));
    press(&mut tool, "y");
    let outcome = tool.on_key(&key(KeyCode::Char('y')));
    assert_eq!(outcome, ToolOutcome::Yank("one".to_string()));
}

#[test]
fn test_the_dot_key_does_the_last_change_again() {
    let tmp = TempDir::new("dot");

    let mut tool = tmp.tool("aaaa\n");
    press(&mut tool, "x..");
    assert_eq!(tool.buffer().lines(), ["a"], "three characters gone");

    let mut typed = tmp.tool("one\ntwo\n");
    type_keys(&mut typed, "Ix <esc>");
    type_keys(&mut typed, "j.");
    assert_eq!(typed.buffer().lines(), ["x one", "x two"], "the typing too");

    // An undo is not a change to be done again.
    let mut undone = tmp.tool("aaaa\n");
    press(&mut undone, "xu.");
    assert_eq!(undone.buffer().lines(), ["aaa"], "the `x`, not the `u`");
}

#[test]
fn test_a_search_goes_to_the_text_and_n_carries_on() {
    let tmp = TempDir::new("search");
    let mut tool = tmp.tool("one two\nthree\ntwo again\n");
    let outcome = tool.on_key(&key(KeyCode::Char('/')));
    let ToolOutcome::Prompt(request) = outcome else {
        panic!("a search asks for what to look for");
    };
    tool.on_prompt(PromptReply {
        answer: Some("two".to_string()),
        tag: request.tag,
    });
    assert_eq!((tool.buffer().row(), tool.buffer().col()), (0, 4));
    press(&mut tool, "n");
    assert_eq!((tool.buffer().row(), tool.buffer().col()), (2, 0));
    press(&mut tool, "N");
    assert_eq!(
        (tool.buffer().row(), tool.buffer().col()),
        (0, 4),
        "back again"
    );
}

#[test]
fn test_the_star_key_looks_for_the_word_under_the_cursor() {
    let tmp = TempDir::new("star");
    let mut tool = tmp.tool("widget\nother\nwidget\n");
    press(&mut tool, "*");
    assert_eq!(tool.buffer().row(), 2);
}

#[test]
fn test_a_mark_is_a_place_to_come_back_to() {
    let tmp = TempDir::new("marks");
    let mut tool = tmp.tool("one\ntwo\nthree\nfour\n");
    press(&mut tool, "jjma");
    press(&mut tool, "gg");
    press(&mut tool, "'a");
    assert_eq!(tool.buffer().row(), 2, "the mark");
    press(&mut tool, "''");
    assert_eq!(tool.buffer().row(), 0, "and back where the jump started");
}

#[test]
fn test_a_click_puts_the_cursor_under_the_pointer_and_a_drag_selects() {
    let tmp = TempDir::new("mouse");
    let mut tool = tmp.tool("one\ntwo\nthree\n");
    paint_frame(&mut tool, 6);
    let gutter = NUMBER_WIDTH + NUMBER_GAP.len();
    tool.on_mouse(Point {
        col: gutter + 2,
        drag: false,
        row: 1,
    });
    assert_eq!((tool.buffer().row(), tool.buffer().col()), (1, 2));
    tool.on_mouse(Point {
        col: gutter + 4,
        drag: true,
        row: 2,
    });
    assert_eq!(tool.modal.mode(), EditMode::Visual, "a drag selects");
    let selection = tool.selection().expect("a selection");
    assert_eq!((selection.start, selection.end), ((1, 2), (2, 4)));
}

#[test]
fn test_the_clipboard_register_asks_the_host_for_what_to_put_back() {
    let tmp = TempDir::new("clipboard-put");
    let mut tool = tmp.tool("one\n");
    tool.on_key(&key(KeyCode::Char('"')));
    tool.on_key(&key(KeyCode::Char('+')));
    let outcome = tool.on_key(&key(KeyCode::Char('p')));
    assert_eq!(
        outcome,
        ToolOutcome::Paste,
        "the page cannot read it itself"
    );
    tool.on_paste("two\n".to_string());
    assert_eq!(
        tool.buffer().lines(),
        ["one", "two"],
        "whole lines, as copied"
    );
}

#[test]
fn test_an_operator_over_a_paragraph_motion_stays_within_the_lines() {
    // `d}` is exclusive and charwise in Vim: it takes to the blank line
    // without taking the blank line itself.
    let tmp = TempDir::new("paragraph-operator");
    let mut tool = tmp.tool("one\ntwo\n\nthree\n");
    press(&mut tool, "d}");
    assert_eq!(tool.buffer().lines(), ["", "three"]);
}

#[test]
fn test_the_wheel_moves_the_view_and_leaves_the_cursor() {
    // Reading further down must not drag the cursor along, or the place
    // being edited is lost the moment the wheel turns.
    let tmp = TempDir::new("wheel");
    let text: String = (0..40).map(|n| format!("line {n}\n")).collect();
    let mut tool = tmp.tool(&text);
    paint_frame(&mut tool, 11);
    tool.on_scroll(-3);
    paint_frame(&mut tool, 11);
    assert_eq!(tool.doc().scroll, 3, "down three");
    assert_eq!(tool.buffer().row(), 0, "the cursor stays on its line");
    tool.on_scroll(2);
    paint_frame(&mut tool, 11);
    assert_eq!(tool.doc().scroll, 1, "and back up two");
    assert_eq!(tool.buffer().row(), 0);
}

fn press_alt(tool: &mut EditorTool, code: KeyCode) -> ToolOutcome {
    tool.on_key(&Key {
        alt: true,
        ..key(code)
    })
}

fn press_ctrl(tool: &mut EditorTool, c: char) -> ToolOutcome {
    tool.on_key(&Key {
        ctrl: true,
        ..key(KeyCode::Char(c))
    })
}

#[test]
fn test_ctrl_slash_comments_the_selected_lines_and_the_cursor_line_back() {
    let tmp = TempDir::new("comment");
    let path = tmp.file("a.py", "x = 1\ny = 2\nz = 3\n");
    let mut tool = EditorTool::new(path, None).expect("opens");
    press(&mut tool, "Vj");
    press_ctrl(&mut tool, '/');
    assert_eq!(tool.buffer().lines(), ["# x = 1", "# y = 2", "z = 3"]);
    assert_eq!(
        tool.modal.mode(),
        EditMode::Normal,
        "the selection is done with"
    );

    // Back on one line, in Insert mode, the caret keeps its character.
    press(&mut tool, "A");
    press_ctrl(&mut tool, '/');
    assert_eq!(tool.buffer().lines()[1], "y = 2");
    assert_eq!(tool.buffer().col(), 5, "still at the end of the line");
}

#[test]
fn test_where_the_cursor_is_goes_to_the_status_bar_not_the_text() {
    let tmp = TempDir::new("status");
    let path = tmp.file("a.txt", "one\ntwo\nthree\n");
    let mut tool = EditorTool::new(path, None).expect("opens");
    press(&mut tool, "jl\"a");
    let status = tool.status();
    assert_eq!(status.mode, Some(EditMode::Normal));
    assert_eq!(status.segments, ["2:2", "3 lines", "\"a"]);
    let first = crate::model::paint::row_text(&tool.content(10, 80, false).rows[0]);
    assert!(
        first.ends_with("one"),
        "the file's first line heads the pane: {first:?}"
    );

    press(&mut tool, "i");
    assert_eq!(tool.status().mode, Some(EditMode::Insert));
}

#[test]
fn test_which_of_several_open_files_this_is_leads_the_status_bar() {
    // The header that said so is gone: the tab names the file, and this is
    // the one thing about it the tab does not say.
    let tmp = TempDir::new("count");
    let first = tmp.file("a.txt", "one\n");
    let second = tmp.file("b.txt", "two\n");
    let mut tool = EditorTool::new(first, None).expect("opens");
    assert_eq!(
        tool.status().segments.first().map(String::as_str),
        Some("1:1")
    );
    assert!(tool.open_file(OpenTarget::file(second)));
    assert_eq!(
        tool.status().segments.first().map(String::as_str),
        Some("2 of 2")
    );
}

#[test]
fn test_a_rebase_plan_sets_actions_by_their_letters_in_normal_mode_only() {
    let tmp = TempDir::new("rebase-letters");
    let path = tmp.file(
        "git-rebase-todo",
        "pick aaa first\npick bbb second\npick ccc third\npick ddd fourth\n",
    );
    let mut tool = EditorTool::new(path, None).expect("opens");

    // Each letter sets its line and steps on to the next.
    press(&mut tool, "rfd");
    assert_eq!(
        tool.buffer().lines()[..3],
        ["reword aaa first", "fixup bbb second", "drop ccc third"]
    );
    assert_eq!(tool.buffer().row(), 3);

    // Typing still types: in Insert mode the letters are text.
    press(&mut tool, "Ae");
    assert_eq!(tool.buffer().lines()[3], "pick ddd fourthe");
}

#[test]
fn test_a_rebase_action_over_a_selection_sets_every_commit_line_in_it() {
    let tmp = TempDir::new("rebase-visual");
    let path = tmp.file(
        "git-rebase-todo",
        "pick aaa first\npick bbb second\n\npick ccc third\npick ddd fourth\n",
    );
    let mut tool = EditorTool::new(path, None).expect("opens");

    press(&mut tool, "jVjjf");
    assert_eq!(
        tool.buffer().lines()[..5],
        [
            "pick aaa first",
            "fixup bbb second",
            "",
            "fixup ccc third",
            "pick ddd fourth",
        ],
        "the blank between is left alone"
    );
    assert_eq!(tool.modal.mode(), EditMode::Normal);
    assert_eq!(tool.buffer().row(), 4, "on past the selection");

    press(&mut tool, "u");
    assert_eq!(
        tool.buffer().lines()[1],
        "pick bbb second",
        "one undo for all"
    );
}

#[test]
fn test_a_rebase_plan_takes_action_chords_and_refuses_to_open_with_a_fold() {
    let tmp = TempDir::new("rebase-todo");
    let path = tmp.file(
        "git-rebase-todo",
        "pick aaa first\npick bbb second\n# help\n",
    );
    let mut tool = EditorTool::new(path.clone(), None).expect("opens");

    // `C-c s` on the first line would fold it into nothing.
    press_ctrl(&mut tool, 'c');
    press(&mut tool, "s");
    press_ctrl(&mut tool, 'c');
    assert_eq!(
        press_ctrl(&mut tool, 'c'),
        ToolOutcome::Consumed,
        "refused, not closed"
    );
    assert!(tool
        .take_notice()
        .map(|notice| notice.text)
        .unwrap_or_default()
        .contains("first commit"));

    // Fix it: setting the action stepped onto the second commit, which
    // moves above the first, and then is dropped.
    press_ctrl(&mut tool, 'c');
    press(&mut tool, "k");
    assert_eq!(tool.buffer().lines()[0], "pick bbb second");
    press_ctrl(&mut tool, 'c');
    press(&mut tool, "d");
    press_ctrl(&mut tool, 'c');
    assert_eq!(press_ctrl(&mut tool, 'c'), ToolOutcome::Close);
    assert_eq!(
        std::fs::read_to_string(&path).expect("written"),
        "drop bbb second\nsquash aaa first\n# help\n"
    );
}

#[test]
fn test_insert_mode_walks_and_kills_by_word_with_the_alt_chords() {
    let tmp = TempDir::new("emacs-words");
    let mut tool = tmp.tool("one two three\n");
    press(&mut tool, "A");
    press_alt(&mut tool, KeyCode::Char('b'));
    assert_eq!(tool.buffer().col(), 8, "back to the start of three");
    press_alt(&mut tool, KeyCode::Char('b'));
    assert_eq!(tool.buffer().col(), 4);
    press_alt(&mut tool, KeyCode::Char('f'));
    assert_eq!(tool.buffer().col(), 7, "forward to the end of two");
    press_alt(&mut tool, KeyCode::Char('d'));
    assert_eq!(tool.buffer().lines(), ["one two"]);
    press_alt(&mut tool, KeyCode::Backspace);
    assert_eq!(tool.buffer().lines(), ["one "]);
    // The two kills ran back to back, so they come back as one.
    press_ctrl(&mut tool, 'y');
    assert_eq!(
        tool.buffer().lines(),
        ["one two three"],
        "the whole run comes back"
    );
}

#[test]
fn test_ctrl_backspace_kills_the_word_behind_the_cursor_as_alt_backspace_does() {
    let tmp = TempDir::new("ctrl-backspace");
    let mut tool = tmp.tool("one two three\n");
    press(&mut tool, "A");
    tool.on_key(&Key {
        ctrl: true,
        ..key(KeyCode::Backspace)
    });
    assert_eq!(tool.buffer().lines(), ["one two "]);
    // Back to back with `M-DEL`, the two kills come back as one.
    press_alt(&mut tool, KeyCode::Backspace);
    assert_eq!(tool.buffer().lines(), ["one "]);
    press_ctrl(&mut tool, 'y');
    assert_eq!(tool.buffer().lines(), ["one two three"]);
}

#[test]
fn test_insert_mode_edits_around_the_cursor_with_the_ctrl_chords() {
    let tmp = TempDir::new("emacs-ctrl");
    let mut tool = tmp.tool("abcd\nxyz\n");
    press(&mut tool, "i");
    press_ctrl(&mut tool, 'f');
    press_ctrl(&mut tool, 'f');
    press_ctrl(&mut tool, 't');
    assert_eq!(tool.buffer().lines(), ["acbd", "xyz"], "transposed");
    press_ctrl(&mut tool, 'd');
    assert_eq!(tool.buffer().lines(), ["acb", "xyz"], "deleted under it");
    press_ctrl(&mut tool, 'h');
    assert_eq!(tool.buffer().lines(), ["ac", "xyz"], "deleted behind it");
    press_ctrl(&mut tool, 'n');
    assert_eq!(tool.buffer().row(), 1, "down a line");
    press_ctrl(&mut tool, 'p');
    assert_eq!(tool.buffer().row(), 0, "and back up");
    press_ctrl(&mut tool, 'o');
    assert_eq!(tool.buffer().lines(), ["ac", "", "xyz"].map(String::from));
    assert_eq!(
        tool.buffer().row(),
        0,
        "the cursor stays ahead of the break"
    );
}

#[test]
fn test_insert_mode_kills_a_line_and_yanks_it_with_ctrl_k_and_ctrl_y() {
    let tmp = TempDir::new("emacs-kill");
    let mut tool = tmp.tool("abc def\n");
    press(&mut tool, "i");
    press_alt(&mut tool, KeyCode::Char('f'));
    press_ctrl(&mut tool, 'k');
    assert_eq!(tool.buffer().lines(), ["abc"]);
    press(&mut tool, "X");
    press_ctrl(&mut tool, 'y');
    assert_eq!(tool.buffer().lines(), ["abcX def"]);
}

#[test]
fn test_the_alt_chords_the_window_keeps_still_reach_it() {
    // `M-h` and `M-l` move focus, and `M-1` picks a pane; claiming
    // them for editing would strand the window chords inside a message.
    let tmp = TempDir::new("emacs-window");
    let mut tool = tmp.tool("abc\n");
    press(&mut tool, "i");
    for c in ['h', 'j', 'k', 'l', '1'] {
        assert_eq!(
            press_alt(&mut tool, KeyCode::Char(c)),
            ToolOutcome::Ignored,
            "{c}"
        );
    }
    assert_eq!(tool.buffer().lines(), ["abc"]);
}

#[test]
fn test_the_space_bar_types_a_space_in_insert_and_after_r() {
    // Regression: the space bar is a key code of its own, which insert
    // mode did not match, so a commit message could not hold a space.
    let tmp = TempDir::new("space");
    let mut tool = tmp.tool("ab\n");
    press(&mut tool, "i");
    tool.on_key(&key(KeyCode::Space));
    press(&mut tool, "c");
    assert_eq!(tool.buffer().lines(), [" cab"]);

    tool.on_key(&key(KeyCode::Escape));
    press(&mut tool, "0lr");
    tool.on_key(&key(KeyCode::Space));
    assert_eq!(
        tool.buffer().lines(),
        ["  ab"],
        "r then space replaces the c"
    );
}

#[test]
fn test_a_key_brings_the_view_back_to_the_cursor() {
    let tmp = TempDir::new("wheel-key");
    let text: String = (0..40).map(|n| format!("line {n}\n")).collect();
    let mut tool = tmp.tool(&text);
    paint_frame(&mut tool, 11);
    tool.on_scroll(-20);
    paint_frame(&mut tool, 11);
    assert!(tool.doc().scroll > 0);
    press(&mut tool, "l");
    paint_frame(&mut tool, 11);
    assert_eq!(tool.doc().scroll, 0, "the cursor is in view again");
}

#[test]
fn test_a_file_changed_underneath_a_clean_editor_is_read_again() {
    let tmp = TempDir::new("resume");
    let mut tool = tmp.tool("before\n");
    std::fs::write(tmp.0.join("a.txt"), "after\n").expect("rewrite");
    // The stamp is the file's mtime, which a second write may share.
    tool.doc_mut().stamp = file::FileStamp::default();
    tool.on_resume();
    assert_eq!(tool.buffer().lines(), ["after"]);
}

#[test]
fn test_a_second_file_opens_as_another_buffer_of_the_same_editor() {
    let tmp = TempDir::new("buffers");
    let mut tool = tmp.tool("one\n");
    let other = tmp.file("b.txt", "two\n");

    tool.open(other.clone(), None);
    assert_eq!(tool.buffer_count(), 2);
    assert_eq!(tool.buffer().lines(), ["two"], "the new one is in front");

    press(&mut tool, "[b");
    assert_eq!(tool.buffer().lines(), ["one"], "`[b` steps back");
    press(&mut tool, "]b");
    assert_eq!(tool.buffer().lines(), ["two"], "and `]b` on again");

    // A file already open is one to go to, not one to open twice.
    press(&mut tool, "[b");
    tool.open(other, None);
    assert_eq!(tool.buffer_count(), 2);
    assert_eq!(tool.buffer().lines(), ["two"]);
}

#[test]
fn test_the_recency_chord_goes_to_the_last_file_worked_on_not_the_next_one() {
    // The whole point of the chord over `]b`: with three files open and
    // the middle one visited last, it goes back to that rather than to
    // whichever file happens to sit beside this one in opening order.
    let tmp = TempDir::new("recent-buffers");
    let mut tool = tmp.tool("one\n");
    tool.open(tmp.file("b.txt", "two\n"), None);
    tool.open(tmp.file("c.txt", "three\n"), None);
    press(&mut tool, "[b");
    assert_eq!(tool.buffer().lines(), ["two"], "visited second");
    press(&mut tool, "]b");
    assert_eq!(tool.buffer().lines(), ["three"], "and back to the third");

    tool.recent_buffer(false);
    assert_eq!(tool.buffer().lines(), ["two"], "the one worked on before");
    tool.recent_buffer(false);
    assert_eq!(tool.buffer().lines(), ["one"], "and further back again");
    tool.recent_buffer(true);
    assert_eq!(tool.buffer().lines(), ["two"], "forward toward the recent");
}

#[test]
fn test_a_walk_does_not_reshuffle_what_it_passes_over() {
    // Walking past a file must not make it the most recent, or a second
    // press would bounce between two files instead of going on back.
    let tmp = TempDir::new("recent-walk");
    let mut tool = tmp.tool("one\n");
    tool.open(tmp.file("b.txt", "two\n"), None);
    tool.open(tmp.file("c.txt", "three\n"), None);
    tool.recent_buffer(false);
    tool.recent_buffer(false);
    assert_eq!(
        tool.buffer().lines(),
        ["one"],
        "two steps back, not a toggle"
    );
}

#[test]
fn test_coming_back_to_a_buffer_re_reads_a_file_written_to_meanwhile() {
    let tmp = TempDir::new("buffer-stale");
    let mut tool = tmp.tool("before\n");
    tool.open(tmp.file("b.txt", "other\n"), None);

    std::fs::write(tmp.0.join("a.txt"), "after\n").expect("rewrite");
    // The stamp is the file's mtime, which a second write may share.
    tool.docs[0].stamp = file::FileStamp::default();

    press(&mut tool, "[b");
    assert_eq!(tool.buffer().lines(), ["after"]);
}

#[test]
fn test_the_buffer_list_goes_to_the_one_chosen() {
    let tmp = TempDir::new("buffer-list");
    let mut tool = tmp.tool("one\n");
    tool.open(tmp.file("b.txt", "two\n"), None);

    tool.on_key(&key(KeyCode::Char('g')));
    let ToolOutcome::Pick(request) = tool.on_key(&key(KeyCode::Char('b'))) else {
        panic!("`gb` asks for one of the open buffers");
    };
    assert_eq!(request.items.len(), 2);
    // Most recently used first: the file in front heads the list, and the
    // one before it follows, so the usual next choice is a row away.
    assert!(request.items[0].ends_with("b.txt"), "the file in front");
    assert!(request.items[1].ends_with("a.txt"), "the one before it");

    let previous = request.items[1].clone();
    tool.on_prompt(PromptReply {
        answer: Some(previous),
        tag: request.tag,
    });
    assert_eq!(tool.buffer().lines(), ["one"]);
}

#[test]
fn test_what_buffers_share_and_what_belongs_to_each_file() {
    // A yank that did not reach the next buffer would make two buffers
    // two editors; a mark that did would name a line in the wrong file.
    let tmp = TempDir::new("shared");
    let mut tool = tmp.tool("one\n");
    press(&mut tool, "mayy");
    tool.open(tmp.file("b.txt", "two\n"), None);

    press(&mut tool, "p");
    assert_eq!(tool.buffer().lines(), ["two", "one"], "the register came");

    press(&mut tool, "'a");
    assert!(
        tool.take_notice()
            .map(|notice| notice.text)
            .unwrap_or_default()
            .contains("not set"),
        "the mark stayed with its own file"
    );
}

#[test]
fn test_a_line_taken_with_dd_comes_back_with_p() {
    let tmp = TempDir::new("dd");
    let mut tool = tmp.tool("one\ntwo\nthree\n");
    press(&mut tool, "dd");
    assert_eq!(tool.buffer().lines(), ["two", "three"]);
    press(&mut tool, "p");
    assert_eq!(tool.buffer().lines(), ["two", "one", "three"]);
}

#[test]
fn test_cw_changes_the_word_without_swallowing_the_space_after_it() {
    // `dw` takes the space and `cw` does not: the difference is the whole
    // reason Vim treats them separately, and getting it wrong runs the
    // changed word into the next one.
    let tmp = TempDir::new("cw");

    let mut change = tmp.tool("alpha beta\n");
    press(&mut change, "cw");
    assert_eq!(change.modal.mode(), EditMode::Insert);
    assert_eq!(change.buffer().lines(), [" beta"]);

    let mut delete = tmp.tool("alpha beta\n");
    press(&mut delete, "dw");
    assert_eq!(delete.buffer().lines(), ["beta"]);
}

#[test]
fn test_the_inclusive_motions_take_the_character_they_land_on() {
    // `d$` leaving the last character behind, and `de` leaving a word's
    // final letter, are the classic off-by-one in an operator.
    let tmp = TempDir::new("inclusive");

    let mut dollar = tmp.tool("keep this\n");
    press(&mut dollar, "ll");
    tool_key(&mut dollar, KeyCode::Char('d'));
    tool_key(&mut dollar, KeyCode::Char('$'));
    assert_eq!(dollar.buffer().lines(), ["ke"]);

    let mut end = tmp.tool("alpha beta\n");
    press(&mut end, "de");
    assert_eq!(end.buffer().lines(), [" beta"]);
}

#[test]
fn test_an_operator_over_a_motion_that_crosses_lines_takes_whole_lines() {
    // `dj` is linewise in Vim: taking only the characters between the two
    // columns would leave two half-lines spliced together.
    let tmp = TempDir::new("linewise");
    let mut tool = tmp.tool("one\ntwo\nthree\n");
    press(&mut tool, "dj");
    assert_eq!(tool.buffer().lines(), ["three"]);
}

#[test]
fn test_a_count_applies_to_the_command_that_follows_it() {
    let tmp = TempDir::new("count");
    let mut tool = tmp.tool("abcdef\n");
    press(&mut tool, "3x");
    assert_eq!(tool.buffer().lines(), ["def"]);

    let mut lines = tmp.tool("one\ntwo\nthree\nfour\n");
    press(&mut lines, "2dd");
    assert_eq!(lines.buffer().lines(), ["three", "four"]);
}

#[test]
fn test_a_leading_zero_is_the_line_start_rather_than_a_count() {
    // Typed after a digit it is part of the count; typed first it is the
    // motion to column one, and reading it as a count of zero would make
    // the next command act ten times or not at all.
    let tmp = TempDir::new("zero");
    let mut tool = tmp.tool("abcdefghijkl\n");
    press(&mut tool, "lll0x");
    assert_eq!(tool.buffer().lines(), ["bcdefghijkl"]);

    let mut ten = tmp.tool("abcdefghijkl\n");
    press(&mut ten, "10x");
    assert_eq!(ten.buffer().lines(), ["kl"]);
}

#[test]
fn test_typing_lands_in_the_text_and_escape_steps_back_onto_it() {
    let tmp = TempDir::new("insert");
    let mut tool = tmp.tool("bc\n");
    press(&mut tool, "i");
    assert_eq!(tool.modal.mode(), EditMode::Insert);
    press(&mut tool, "a");
    assert_eq!(tool.buffer().lines(), ["abc"]);

    tool.on_key(&key(KeyCode::Escape));
    assert_eq!(tool.modal.mode(), EditMode::Normal);
    assert_eq!(tool.buffer().col(), 0, "back onto the character just typed");
}

#[test]
fn test_saving_writes_the_text_and_clears_the_unsaved_mark() {
    let tmp = TempDir::new("save");
    let mut tool = tmp.tool("one\ntwo\n");
    press(&mut tool, "x");
    assert!(tool.buffer().is_dirty());

    tool.on_key(&ctrl('s'));
    assert!(!tool.buffer().is_dirty());
    let written = std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads");
    assert_eq!(written, "ne\ntwo\n");
}

#[test]
fn test_saving_while_typing_goes_back_to_normal_as_escape_would() {
    let tmp = TempDir::new("save-normal");
    let mut tool = tmp.tool("one\n");
    press(&mut tool, "A!");
    assert_eq!(tool.modal.mode(), EditMode::Insert);

    tool.on_key(&ctrl('s'));
    assert_eq!(tool.modal.mode(), EditMode::Normal);
    assert_eq!(tool.buffer().col(), 3, "onto the character just typed");
    assert_eq!(
        std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads"),
        "one!\n"
    );
}

#[test]
fn test_saving_mid_selection_lets_the_selection_go() {
    let tmp = TempDir::new("save-visual");
    let mut tool = tmp.tool("one\n");
    press(&mut tool, "vl");
    assert_eq!(tool.modal.mode(), EditMode::Visual);

    tool.on_key(&ctrl('s'));
    assert_eq!(tool.modal.mode(), EditMode::Normal);
}

#[test]
fn test_ctrl_shift_s_saves_as_ctrl_s_does_rather_than_leaving_it_to_grep() {
    let tmp = TempDir::new("save-shift");
    let mut tool = tmp.tool("one\n");
    press(&mut tool, "x");

    let outcome = tool.on_key(&Key {
        shift: true,
        ..ctrl('s')
    });
    assert!(
        !matches!(outcome, ToolOutcome::Ignored),
        "the editor keeps it"
    );
    assert!(!tool.buffer().is_dirty());
    let written = std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads");
    assert_eq!(written, "ne\n");
}

#[test]
fn test_saving_over_a_file_changed_underneath_asks_first() {
    // Overwriting silently is the one thing an editor does that cannot be
    // undone from inside it.
    let tmp = TempDir::new("stale");
    let mut tool = tmp.tool("one\n");
    press(&mut tool, "x");
    std::fs::write(tmp.0.join("a.txt"), "written elsewhere\n").expect("outside write");

    let outcome = tool.on_key(&ctrl('s'));
    let ToolOutcome::Prompt(request) = outcome else {
        panic!("expected a question, got {outcome:?}");
    };
    assert_eq!(request.tag, ASK_OVERWRITE);
    assert_eq!(
        std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads"),
        "written elsewhere\n",
        "nothing was written while the question was open"
    );
}

fn message_tool(tmp: &TempDir) -> (EditorTool, PathBuf) {
    let path = tmp.0.join("COMMIT_EDITMSG");
    std::fs::write(&path, "\n# Please enter\n").expect("writes");
    (EditorTool::new(path.clone(), None).expect("opens"), path)
}

#[test]
fn test_a_commit_message_starts_on_its_first_line_with_no_header_above_it() {
    let tmp = TempDir::new("headerless");
    let (mut tool, _) = message_tool(&tmp);
    let content = tool.content(10, 60, false);
    assert!(
        crate::model::paint::row_text(&content.rows[1]).contains("# Please enter"),
        "the file's second line is the frame's second row"
    );
    tool.on_mouse(Point {
        col: NUMBER_WIDTH + NUMBER_GAP.len() + 2,
        drag: false,
        row: 1,
    });
    assert_eq!(
        tool.buffer().row(),
        1,
        "a click lands on the line drawn there"
    );
}

#[test]
fn test_a_commit_message_opens_ready_to_type_and_c_c_c_c_saves_and_closes() {
    let tmp = TempDir::new("finish");
    let (mut tool, path) = message_tool(&tmp);
    assert_eq!(tool.modal.mode(), EditMode::Insert);

    press(&mut tool, "fix");
    assert_eq!(tool.on_key(&ctrl('c')), ToolOutcome::Consumed);
    assert_eq!(tool.on_key(&ctrl('c')), ToolOutcome::Close);
    assert_eq!(
        std::fs::read_to_string(path).expect("reads"),
        "fix\n# Please enter\n"
    );
}

#[test]
fn test_ctrl_shift_c_saves_and_closes_a_commit_message() {
    let tmp = TempDir::new("ctrl-shift-c");
    let (mut tool, path) = message_tool(&tmp);
    press(&mut tool, "fix");
    // The window reports the capital with Shift held, which is what a
    // real key press looks like.
    let chord = Key {
        code: KeyCode::Char('C'),
        shift: true,
        sup: false,
        ..ctrl('c')
    };
    assert_eq!(tool.on_key(&chord), ToolOutcome::Close);
    assert_eq!(
        std::fs::read_to_string(&path).expect("reads"),
        "fix\n# Please enter\n"
    );

    let (mut tool, _) = message_tool(&tmp);
    let lower = Key {
        shift: true,
        sup: false,
        ..ctrl('c')
    };
    assert_eq!(tool.on_key(&lower), ToolOutcome::Close);
}

#[test]
fn test_c_c_c_k_and_zq_empty_a_commit_message_so_git_aborts() {
    let tmp = TempDir::new("cancel");
    let (mut tool, path) = message_tool(&tmp);
    press(&mut tool, "fix");
    tool.on_key(&ctrl('c'));
    assert_eq!(tool.on_key(&ctrl('k')), ToolOutcome::Close);
    assert_eq!(std::fs::read_to_string(&path).expect("reads"), "\n");

    let (mut tool, path) = message_tool(&tmp);
    press(&mut tool, "x");
    tool.on_key(&key(KeyCode::Escape));
    press(&mut tool, "ZQ");
    assert_eq!(std::fs::read_to_string(path).expect("reads"), "\n");
}

#[test]
fn test_a_lone_ctrl_c_is_forgotten_by_the_next_key() {
    let tmp = TempDir::new("chord");
    let (mut tool, path) = message_tool(&tmp);
    tool.on_key(&ctrl('c'));
    press(&mut tool, "a");
    assert_eq!(tool.on_key(&ctrl('c')), ToolOutcome::Consumed);
    assert_eq!(
        std::fs::read_to_string(path).expect("reads"),
        "\n# Please enter\n",
        "nothing was saved"
    );
}

#[test]
fn test_the_first_recall_asks_git_and_lands_when_it_answers() {
    // The history is read as a job, so the window never waits on git;
    // the step back the key asked for is taken when the answer comes.
    let tmp = TempDir::new("recall-job");
    let path = tmp.0.join("COMMIT_EDITMSG");
    std::fs::write(&path, "\n# Please enter\n").expect("writes");
    let mut tool = EditorTool::new(path, None).expect("opens");
    let ToolOutcome::Job(crate::model::tool::JobRequest::Command(request)) =
        tool.recall_message(true)
    else {
        panic!("expected the history to be asked for");
    };
    assert_eq!(request.tag, exec::TAG_MESSAGES);
    assert_eq!(tool.buffer().lines()[0], "", "nothing swapped yet");
    tool.on_job(JobReply::Command(crate::model::tool::CommandOutput {
        code: Some(0),
        stderr: String::new(),
        stdout: "earlier message\u{1e}".to_string(),
        tag: exec::TAG_MESSAGES,
    }));
    assert_eq!(tool.buffer().lines()[0], "earlier message");
}

#[test]
fn test_recalling_swaps_only_the_message_above_the_footer() {
    let tmp = TempDir::new("recall");
    let path = tmp.0.join("COMMIT_EDITMSG");
    std::fs::write(&path, "\n# Please enter\n# a message\n").expect("writes");
    let mut tool = EditorTool::new(path, None).expect("opens");
    tool.recall = MessageRecall::with_history(&["old", "new"]);

    tool.recall_message(true);
    assert_eq!(
        tool.buffer().lines(),
        ["new", "# Please enter", "# a message"]
    );
    tool.recall_message(true);
    assert_eq!(tool.buffer().lines()[0], "old");
    tool.recall_message(false);
    tool.recall_message(false);
    assert_eq!(
        tool.buffer().lines(),
        ["", "# Please enter", "# a message"],
        "stepping past the newest restores what was typed"
    );
}

#[test]
fn test_zz_writes_and_closes_while_zq_closes_without_writing() {
    let tmp = TempDir::new("leave");

    let mut written = tmp.tool("one\n");
    press(&mut written, "x");
    press(&mut written, "ZZ");
    assert_eq!(
        std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads"),
        "ne\n"
    );

    let mut discarded = tmp.tool("one\n");
    press(&mut discarded, "x");
    assert_eq!(
        discarded.on_key(&key(KeyCode::Char('Z'))),
        ToolOutcome::Consumed
    );
    assert_eq!(
        discarded.on_key(&key(KeyCode::Char('Q'))),
        ToolOutcome::Close
    );
    assert_eq!(
        std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads"),
        "one\n",
        "the edits were dropped, not written"
    );
}

#[test]
fn test_the_file_opens_on_the_line_the_tool_was_pointing_at() {
    // A grep hit opened at line one throws away what was searched for.
    let tmp = TempDir::new("line");
    let path = tmp.0.join("a.txt");
    std::fs::write(&path, "one\ntwo\nthree\n").expect("temp file");
    let tool = EditorTool::new(path, Some(3)).expect("opens");
    assert_eq!(tool.buffer().row(), 2);
}

#[test]
fn test_handing_off_to_the_outside_editor_saves_first_and_carries_the_line() {
    // `$EDITOR` reads the file from disk, so unsaved edits would not be
    // there; landing it at line one would lose the place as well.
    let tmp = TempDir::new("escalate");
    let mut tool = tmp.tool("one\ntwo\nthree\n");
    press(&mut tool, "jx");
    let outcome = tool.on_key(&ctrl('o'));
    assert_eq!(
        outcome,
        ToolOutcome::SpawnEditor(OpenTarget::at_line(tmp.0.join("a.txt"), 2))
    );
    assert_eq!(
        std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads"),
        "one\nwo\nthree\n"
    );
}

#[test]
fn test_the_window_chords_still_reach_the_window_from_either_mode() {
    // An Alt chord read as text types an `h` instead of moving focus, and
    // in Normal mode `M-p` would paste: a tool that swallows the window
    // chords traps the user inside it.
    let tmp = TempDir::new("alt");
    let alt = |c: char| Key {
        alt: true,
        code: KeyCode::Char(c),
        ctrl: false,
        shift: false,
        sup: false,
    };

    let mut tool = tmp.tool("one\ntwo\n");
    assert_eq!(tool.on_key(&alt('h')), ToolOutcome::Ignored);
    press(&mut tool, "dd");
    assert_eq!(tool.on_key(&alt('p')), ToolOutcome::Ignored);
    assert_eq!(tool.buffer().lines(), ["two"], "nothing was pasted");

    press(&mut tool, "i");
    assert_eq!(tool.on_key(&alt('h')), ToolOutcome::Ignored);
    assert_eq!(tool.buffer().lines(), ["two"], "and nothing was typed");

    // The two buffer ends the shared layer claims are still the tool's.
    let mut ends = tmp.tool("one\ntwo\nthree\n");
    let shifted = Key {
        alt: true,
        code: KeyCode::Char('>'),
        ctrl: false,
        shift: true,
        sup: false,
    };
    assert_eq!(ends.on_key(&shifted), ToolOutcome::Consumed);
    assert_eq!(ends.buffer().row(), 2);
}

#[test]
fn test_painting_survives_a_pane_with_no_room_and_a_line_with_no_text() {
    // Every one of these indexes the line the cursor is on to draw the
    // cursor into it, and an empty line, a pane one column wide, and a
    // pane with no rows at all are where that goes wrong.
    let tmp = TempDir::new("paint");
    let mut tool = tmp.tool("one\n\nthree\n");
    for (rows, cols) in [(0, 0), (1, 1), (2, 6), (40, 80)] {
        tool.content(rows, cols, false);
        tool.content(rows, cols, true);
    }

    // On the empty line, in both modes, and past the end of a short one.
    press(&mut tool, "j");
    tool.content(10, 20, false);
    press(&mut tool, "i");
    tool.content(10, 20, false);
}

/// The styles of one painted text row, run by run, with the line-number
/// gutter dropped.
fn painted(tool: &mut EditorTool, row: usize) -> Vec<(Style, String)> {
    let content = tool.content(20, 60, false);
    content.rows[row]
        .iter()
        .skip(1)
        .map(|span| (span.style, span.text.clone()))
        .collect()
}

/// The style text whose color and weight both come from the scope table's
/// row for `selector` is painted in.
fn scope(selector: &str) -> Style {
    let rules = winter_render::SCOPE_RULES;
    let row = rules
        .iter()
        .position(|rule| rule.scope == selector)
        .expect("a row of the scope table");
    Style::Syntax(SyntaxStyle {
        color: rules[row].colors().then_some(row as u16),
        weight: rules[row].weighs().then_some(row as u16),
    })
}

/// The scope table row a painted style takes its color from.
fn color_of(style: Style) -> Option<&'static str> {
    match style {
        Style::Syntax(rows) => rows
            .color
            .map(|row| winter_render::SCOPE_RULES[row as usize].scope),
        _ => None,
    }
}

/// The style the first character of `text` is painted in, on text row `row`.
fn style_at(tool: &mut EditorTool, row: usize, text: &str) -> Style {
    let cells: Vec<(char, Style)> = painted(tool, row)
        .into_iter()
        .flat_map(|(style, run)| run.chars().map(move |c| (c, style)).collect::<Vec<_>>())
        .collect();
    let line: String = cells.iter().map(|(c, _)| c).collect();
    let at = line.find(text).expect("the text is on the row");
    cells[line[..at].chars().count()].1
}

#[test]
fn test_source_is_painted_by_what_each_run_of_it_is() {
    let tmp = TempDir::new("syntax");
    let path = tmp.file("a.rs", "let n = 42; // why\n");
    let mut tool = EditorTool::new(path, None).expect("opens");

    assert_eq!(style_at(&mut tool, 0, "let"), scope("storage.type"));
    assert_eq!(style_at(&mut tool, 0, "n "), Style::Normal);
    assert_eq!(style_at(&mut tool, 0, "42"), scope("constant.numeric"));
    assert_eq!(style_at(&mut tool, 0, "why"), scope("comment"));
}

#[test]
fn test_git_s_own_files_are_painted_by_git_s_grammars() {
    let tmp = TempDir::new("git-colors");
    let plan = tmp.file("git-rebase-todo", "pick abc1 subject\n");
    let mut tool = EditorTool::new(plan, None).expect("opens");
    assert_eq!(style_at(&mut tool, 0, "pick"), scope("keyword.operator"));

    let summary = format!("feat: x{}\n\n# a comment\n", "y".repeat(80));
    let message = tmp.file("COMMIT_EDITMSG", &summary);
    let mut tool = EditorTool::new(message, None).expect("opens");
    assert_eq!(
        color_of(style_at(&mut tool, 0, "feat")),
        Some("keyword"),
        "keyword-colored, and as heavy as the summary around it"
    );
    assert_eq!(
        style_at(&mut tool, 0, "yyyy"),
        scope("markup.heading.subject.git.commit")
    );
    tool.content(20, 60, false);
    let summary = tool.colors_of(0).to_vec();
    assert_eq!(
        (
            summary[commit::SUMMARY_WIDTH - 1],
            summary[commit::SUMMARY_WIDTH]
        ),
        (scope("markup.heading.subject.git.commit"), Style::Removed),
        "past the width a summary reads well at"
    );
    assert_eq!(style_at(&mut tool, 2, "a comment"), scope("comment"));

    let verbose = tmp.file(
        "MERGE_MSG",
        "x\n# ------------------------ >8 ------------------------\n\
         diff --git a/f b/f\n+added\n",
    );
    let mut tool = EditorTool::new(verbose, None).expect("opens");
    assert_eq!(
        style_at(&mut tool, 3, "added"),
        scope("markup.inserted.diff"),
        "the diff below the scissors"
    );

    let notes = tmp.file("notes.md", "```\n# code\n```\n# Title\n");
    let mut tool = EditorTool::new(notes, None).expect("opens");
    assert_eq!(
        style_at(&mut tool, 1, "code"),
        scope("markup.raw.code-fence"),
        "inside the fence"
    );
    assert_eq!(style_at(&mut tool, 3, "Title"), scope("markup.heading.1"));
}

#[test]
fn test_show_scopes_says_what_the_grammar_makes_of_the_cursor_s_character() {
    let tmp = TempDir::new("scopes");
    let mut tool = EditorTool::new(tmp.file("a.rs", "let x = 1;\n"), None).expect("opens");
    let outcome = tool.run_command(SHOW_SCOPES);
    assert!(matches!(outcome, Some(ToolOutcome::Consumed)));
    let notice = tool.take_notice().expect("a notice");
    assert!(notice.text.contains("storage.type.rust"), "{}", notice.text);
    assert!(tool.run_command("no_such_command").is_none());
}

#[test]
fn test_a_script_without_an_extension_is_known_by_its_shebang() {
    let tmp = TempDir::new("shebang");
    let path = tmp.file("deploy", "#!/usr/bin/env python3\nimport os\n");
    let mut tool = EditorTool::new(path, None).expect("opens");
    assert_eq!(style_at(&mut tool, 1, "import"), scope("keyword.control"));
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

    let mut whole = EditorTool::new(path.clone(), None).expect("opens");
    whole.doc_mut().color_upto = 40;
    whole.doc_mut().learn_states(Duration::MAX);

    let mut stepped = EditorTool::new(path, None).expect("opens");
    stepped.doc_mut().color_upto = 40;
    stepped.doc_mut().learn_states(Duration::ZERO);
    assert!(stepped.doc().learning(), "one line per stretch");
    assert!(stepped.next_tick().is_some(), "so it asks to be ticked");
    for _ in 0..40 {
        stepped.doc_mut().learn_states(Duration::ZERO);
    }
    assert!(!stepped.doc().learning());
    assert!(stepped.next_tick().is_none());
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
    let mut tool = EditorTool::new(path, None).expect("opens");

    assert_eq!(
        painted(&mut tool, 0),
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
    let mut tool = EditorTool::new(path, None).expect("opens");

    press(&mut tool, "G");
    tool.content(20, 60, false);
    let last = tool.buffer().lines().len() - 1;
    let visible = last - tool.doc().scroll;
    let styles: Vec<Style> = painted(&mut tool, visible - 1)
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
    let mut tool = EditorTool::new(path, None).expect("opens");
    assert_eq!(painted(&mut tool, 1)[0].0, scope("storage.type"));

    // Open a comment on the first line; the second is now inside it.
    press(&mut tool, "I");
    press(&mut tool, "/*");
    assert!(
        painted(&mut tool, 1)
            .iter()
            .all(|(style, _)| *style == scope("comment")),
        "the line below was left as code"
    );
}

#[test]
fn test_the_caret_lands_on_the_text_rather_than_in_the_line_numbers() {
    // The host puts a tool's cursor on the row's first painted character
    // unless the tool says otherwise, and here that is the line number.
    let tmp = TempDir::new("caret");
    let mut tool = tmp.tool("fn main() {\n\tlet x = 1;\n");
    let gutter = NUMBER_WIDTH + NUMBER_GAP.len();

    tool.content(10, 40, false);
    let caret = tool.caret().expect("a caret");
    assert_eq!(caret.col, gutter, "column one of the text, not of the row");
    assert!(!caret.insert);

    // A tab is one character in the file and several columns on screen,
    // so a caret that counted characters would drift left of the text.
    press(&mut tool, "jll");
    tool.content(10, 40, false);
    assert_eq!(
        tool.caret().expect("a caret").col,
        gutter + TAB_WIDTH + 1,
        "past the tab's width, then one character"
    );

    // Typing says so, so the caret can take the shape typing has.
    press(&mut tool, "i");
    assert!(tool.caret().expect("a caret").insert);
}

#[test]
fn test_a_line_wider_than_the_pane_scrolls_to_keep_the_cursor_in_view() {
    // Without this, editing past the pane's width types into a column
    // nobody can see.
    let tmp = TempDir::new("hscroll");
    let mut tool = tmp.tool(&format!("{}\n", "x".repeat(200)));
    tool.content(10, 40, false);
    assert_eq!(tool.doc().hscroll, 0);

    press(&mut tool, "$");
    tool.content(10, 40, false);
    assert!(
        tool.doc().hscroll > 0,
        "the end of the line was scrolled to"
    );

    press(&mut tool, "0");
    tool.content(10, 40, false);
    assert_eq!(tool.doc().hscroll, 0, "and back again");
}

#[test]
fn test_editor_jump_targets_preview_confirm_and_cancel() {
    let tmp = TempDir::new("editor_jump");
    let mut tool = tmp.tool("first line\n\nthird line\nfourth line\n");
    let targets = tool.jump_targets().expect("jump targets");
    assert_eq!(targets.len(), 3);
    assert_eq!(targets[0].target, 0);
    assert_eq!(targets[1].target, 2);
    assert_eq!(targets[2].target, 3);
    assert_eq!(tool.jump_cursor(), Some(0));

    // Live preview to line 2
    tool.jump_to(2);
    assert_eq!(tool.buffer().row(), 2);

    // Cancel restores cursor to line 0
    tool.jump_cancel();
    assert_eq!(tool.buffer().row(), 0);

    // With painted viewport (e.g. 10 rows), preview centers at viewport / 2
    tool.content(10, 40, false);
    tool.jump_to(3);
    assert_eq!(tool.buffer().row(), 3);
    assert_eq!(tool.doc().scroll, 3_usize.saturating_sub(10 / 2));

    // Preview to line 3 and confirm
    tool.jump_confirm(3);
    assert_eq!(tool.buffer().row(), 3);
    assert_eq!(tool.doc().scroll, 3_usize.saturating_sub(10 / 2));
    assert_eq!(tool.doc().marks.jump, Some((0, 0)));
}

fn tool_key(tool: &mut EditorTool, code: KeyCode) {
    tool.on_key(&key(code));
}

#[test]
fn test_the_caret_counts_the_cells_wide_and_combining_characters_take() {
    // Regression: every character but a tab counted as one cell, so past
    // a CJK character the caret sat a cell left of the text.
    let tmp = TempDir::new("caret-wide");
    let mut tool = tmp.tool("\u{4e2d}e\u{301}x\n");
    let gutter = NUMBER_WIDTH + NUMBER_GAP.len();
    press(&mut tool, "ll");
    tool.content(10, 40, false);
    assert_eq!(
        tool.caret().expect("a caret").col,
        gutter + 3,
        "two cells for the wide one, one for the accented e"
    );
    assert_eq!(tool.char_at_display(0, 1), 0, "either half of it is it");
    assert_eq!(tool.char_at_display(0, 2), 1);
}

#[cfg(unix)]
#[test]
fn test_a_file_the_user_cannot_write_asks_to_save_as_root_and_stays_open() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new("as-root");
    let mut tool = tmp.tool("one\n");
    let path = tmp.0.join("a.txt");
    let locked = |mode| std::fs::set_permissions(&tmp.0, std::fs::Permissions::from_mode(mode));
    locked(0o555).expect("read-only directory");
    // Root writes anywhere, so there is nothing to ask it for.
    if std::fs::write(tmp.0.join("probe"), "").is_ok() {
        locked(0o755).expect("writable again");
        return;
    }
    press(&mut tool, "x");
    tool.on_key(&key(KeyCode::Char('Z')));
    let outcome = tool.on_key(&key(KeyCode::Char('Z')));
    locked(0o755).expect("writable again");

    let ToolOutcome::Prompt(request) = outcome else {
        panic!("ZZ asks rather than closing over the edit: {outcome:?}");
    };
    assert_eq!(request.tag, ASK_AS_ROOT);
    assert_eq!(request.mode, PromptMode::Confirm);
    assert!(tool.is_dirty(), "the edit is still held");
    assert_eq!(std::fs::read_to_string(&path).expect("reads"), "one\n");
}

#[test]
fn test_an_unread_buffer_reads_its_file_before_taking_any_edit() {
    let tmp = TempDir::new("unread");
    let path = tmp.file("a.txt", "one\ntwo\n");
    let unread = || EditorTool::with_doc(Document::unread(path.clone(), Some(2)), EditMode::Normal);

    let mut keyed = unread();
    keyed.on_key(&key(KeyCode::Char('x')));
    assert_eq!(
        keyed.buffer().lines(),
        ["one", "two"],
        "the key read, and edited nothing"
    );
    assert_eq!(keyed.buffer().row(), 1, "at the line it was restored on");

    let mut pasted = unread();
    pasted.on_paste("pasted".to_string());
    assert_eq!(pasted.buffer().lines(), ["one", "two"]);

    let mut saved = unread();
    saved.write();
    assert_eq!(
        std::fs::read_to_string(&path).expect("reads"),
        "one\ntwo\n",
        "a save reads first rather than writing the empty stand-in over the file"
    );
    assert!(!saved.is_dirty());
}

#[test]
fn test_a_restored_remote_buffer_waits_to_be_shown_before_reaching_out() {
    // Nothing is run: a host that is not there would otherwise hold the
    // restart until it timed out.
    let tool = EditorTool::restored(PathBuf::from("ssh://nowhere.invalid/etc/motd"), Some(3))
        .expect("restores without reading");
    assert!(tool.doc().unread);
    assert_eq!(tool.title(), "motd");
    assert_eq!(
        tool.cwd(),
        None,
        "no local shell can start in a remote directory"
    );
}

/// Each entry of `tool`'s menu that is not a separator, by label.
fn menu_labels(tool: &EditorTool) -> Vec<String> {
    tool.context_items()
        .into_iter()
        .filter(|item| !item.is_separator())
        .map(|item| item.label)
        .collect()
}

/// Type the keys of `tool`'s menu entry `label`, and hand back what the last
/// one did.
fn choose(tool: &mut EditorTool, label: &str) -> ToolOutcome {
    let item = tool
        .context_items()
        .into_iter()
        .find(|item| item.label == label)
        .unwrap_or_else(|| panic!("{label} in {:?}", menu_labels(tool)));
    let mut last = ToolOutcome::Ignored;
    for key in &item.keys {
        last = tool.on_key(key);
    }
    last
}

#[test]
fn test_the_menu_copies_a_selection_to_the_clipboard_and_keeps_it_on_right_click() {
    let tmp = TempDir::new("menu-copy");
    let mut tool = tmp.tool("alpha beta\n");
    paint_frame(&mut tool, 10);
    press(&mut tool, "ve");
    tool.on_context_mouse(Point {
        col: 30,
        drag: false,
        row: 0,
    });
    assert!(tool.modal.is_selecting(), "a right-click keeps the selection");
    assert_eq!(menu_labels(&tool), ["Copy", "Cut", "Toggle Comment"]);
    assert_eq!(choose(&mut tool, "Copy"), ToolOutcome::Yank("alpha".to_string()));
}

#[test]
fn test_the_menu_pastes_from_the_clipboard_in_normal_mode() {
    let tmp = TempDir::new("menu-paste");
    let mut tool = tmp.tool("alpha\n");
    let labels = menu_labels(&tool);
    assert_eq!(labels.first().map(String::as_str), Some("Paste"));
    assert!(labels.contains(&"Undo".to_string()) && labels.contains(&"Save".to_string()));
    assert_eq!(choose(&mut tool, "Paste"), ToolOutcome::Paste);
}

#[test]
fn test_every_normal_mode_menu_entry_is_a_key_the_editor_answers() {
    let tmp = TempDir::new("menu-keys");
    for label in menu_labels(&tmp.tool("alpha\n")) {
        let mut tool = tmp.tool("alpha\n");
        assert_ne!(choose(&mut tool, &label), ToolOutcome::Ignored, "{label}");
    }
}

#[test]
fn test_insert_mode_offers_only_what_does_not_type() {
    let tmp = TempDir::new("menu-insert");
    let mut tool = tmp.tool("alpha\n");
    press(&mut tool, "i");
    assert_eq!(menu_labels(&tool), ["Toggle Comment", "Save"]);
}
