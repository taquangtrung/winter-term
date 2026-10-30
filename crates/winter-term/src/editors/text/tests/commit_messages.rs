//! The text editor's tests: commit messages and rebase plans.

use super::*;

#[test]
fn test_a_rebase_plan_sets_actions_by_their_letters_in_normal_mode_only() {
    let tmp = TempDir::new("rebase-letters");
    let path = tmp.file(
        "git-rebase-todo",
        "pick aaa first\npick bbb second\npick ccc third\npick ddd fourth\n",
    );
    let mut editor = TextEditor::new(path, None).expect("opens");

    // Each letter sets its line and steps on to the next.
    press(&mut editor, "rfd");
    assert_eq!(
        editor.buffer().lines()[..3],
        ["reword aaa first", "fixup bbb second", "drop ccc third"]
    );
    assert_eq!(editor.buffer().row(), 3);

    // Typing still types: in Insert mode the letters are text.
    press(&mut editor, "Ae");
    assert_eq!(editor.buffer().lines()[3], "pick ddd fourthe");
}

#[test]
fn test_a_rebase_action_over_a_selection_sets_every_commit_line_in_it() {
    let tmp = TempDir::new("rebase-visual");
    let path = tmp.file(
        "git-rebase-todo",
        "pick aaa first\npick bbb second\n\npick ccc third\npick ddd fourth\n",
    );
    let mut editor = TextEditor::new(path, None).expect("opens");

    press(&mut editor, "jVjjf");
    assert_eq!(
        editor.buffer().lines()[..5],
        [
            "pick aaa first",
            "fixup bbb second",
            "",
            "fixup ccc third",
            "pick ddd fourth",
        ],
        "the blank between is left alone"
    );
    assert_eq!(editor.modal.mode(), EditMode::Normal);
    assert_eq!(editor.buffer().row(), 4, "on past the selection");

    press(&mut editor, "u");
    assert_eq!(
        editor.buffer().lines()[1],
        "pick bbb second",
        "one undo for all"
    );
}

/// `C-S-<c>` as the window reports it: the capital, with Shift held.
fn ctrl_shift(c: char) -> Key {
    Key {
        code: KeyCode::Char(c.to_ascii_uppercase()),
        shift: true,
        sup: false,
        ..ctrl(c)
    }
}

#[test]
fn test_ctrl_shift_k_empties_a_commit_message_so_git_aborts() {
    let tmp = TempDir::new("ctrl-shift-k");
    let (mut editor, path) = message_editor(&tmp);
    press(&mut editor, "fix");
    assert_eq!(editor.on_key(&ctrl_shift('k')), EditorOutcome::Close);
    assert_eq!(std::fs::read_to_string(&path).expect("reads"), "\n");
}

#[test]
fn test_ctrl_shift_c_and_k_carry_out_or_abort_a_rebase_plan() {
    let tmp = TempDir::new("rebase-ctrl-shift");
    let plan = "pick aaa first\npick bbb second\n# help\n";
    let path = tmp.file("git-rebase-todo", plan);
    let mut editor = TextEditor::new(path.clone(), None).expect("opens");
    assert_eq!(editor.on_key(&ctrl_shift('c')), EditorOutcome::Close);
    assert_eq!(std::fs::read_to_string(&path).expect("written"), plan, "carried out as it stands");

    let mut editor = TextEditor::new(path.clone(), None).expect("opens");
    assert_eq!(editor.on_key(&ctrl_shift('k')), EditorOutcome::Close);
    assert_eq!(std::fs::read_to_string(&path).expect("written").trim(), "", "an empty plan aborts");
}

#[test]
fn test_a_rebase_plan_takes_action_chords_and_refuses_to_open_with_a_fold() {
    let tmp = TempDir::new("rebase-todo");
    let path = tmp.file(
        "git-rebase-todo",
        "pick aaa first\npick bbb second\n# help\n",
    );
    let mut editor = TextEditor::new(path.clone(), None).expect("opens");

    // `C-c s` on the first line would fold it into nothing.
    press_ctrl(&mut editor, 'c');
    press(&mut editor, "s");
    press_ctrl(&mut editor, 'c');
    assert_eq!(
        press_ctrl(&mut editor, 'c'),
        EditorOutcome::Consumed,
        "refused, not closed"
    );
    assert!(editor
        .take_notice()
        .map(|notice| notice.text)
        .unwrap_or_default()
        .contains("first commit"));

    // Fix it: setting the action stepped onto the second commit, which
    // moves above the first, and then is dropped.
    press_ctrl(&mut editor, 'c');
    press(&mut editor, "k");
    assert_eq!(editor.buffer().lines()[0], "pick bbb second");
    press_ctrl(&mut editor, 'c');
    press(&mut editor, "d");
    press_ctrl(&mut editor, 'c');
    assert_eq!(press_ctrl(&mut editor, 'c'), EditorOutcome::Close);
    assert_eq!(
        std::fs::read_to_string(&path).expect("written"),
        "drop bbb second\nsquash aaa first\n# help\n"
    );
}

#[test]
fn test_a_commit_message_starts_on_its_first_line_with_no_header_above_it() {
    let tmp = TempDir::new("headerless");
    let (mut editor, _) = message_editor(&tmp);
    let content = editor.content(10, 60, false);
    assert!(
        crate::model::paint::row_text(&content.rows[1]).contains("# Please enter"),
        "the file's second line is the frame's second row"
    );
    editor.on_mouse(Point {
        col: NUMBER_WIDTH + NUMBER_GAP.len() + 2,
        drag: false,
        row: 1,
    });
    assert_eq!(
        editor.buffer().row(),
        1,
        "a click lands on the line drawn there"
    );
}

#[test]
fn test_a_commit_message_opens_ready_to_type_and_c_c_c_c_saves_and_closes() {
    let tmp = TempDir::new("finish");
    let (mut editor, path) = message_editor(&tmp);
    assert_eq!(editor.modal.mode(), EditMode::Insert);

    press(&mut editor, "fix");
    assert_eq!(editor.on_key(&ctrl('c')), EditorOutcome::Consumed);
    assert_eq!(editor.on_key(&ctrl('c')), EditorOutcome::Close);
    assert_eq!(
        std::fs::read_to_string(path).expect("reads"),
        "fix\n# Please enter\n"
    );
}

#[test]
fn test_ctrl_shift_c_saves_and_closes_a_commit_message() {
    let tmp = TempDir::new("ctrl-shift-c");
    let (mut editor, path) = message_editor(&tmp);
    press(&mut editor, "fix");
    // The window reports the capital with Shift held, which is what a
    // real key press looks like.
    let chord = Key {
        code: KeyCode::Char('C'),
        shift: true,
        sup: false,
        ..ctrl('c')
    };
    assert_eq!(editor.on_key(&chord), EditorOutcome::Close);
    assert_eq!(
        std::fs::read_to_string(&path).expect("reads"),
        "fix\n# Please enter\n"
    );

    let (mut editor, _) = message_editor(&tmp);
    let lower = Key {
        shift: true,
        sup: false,
        ..ctrl('c')
    };
    assert_eq!(editor.on_key(&lower), EditorOutcome::Close);
}

#[test]
fn test_c_c_c_k_and_zq_empty_a_commit_message_so_git_aborts() {
    let tmp = TempDir::new("cancel");
    let (mut editor, path) = message_editor(&tmp);
    press(&mut editor, "fix");
    editor.on_key(&ctrl('c'));
    assert_eq!(editor.on_key(&ctrl('k')), EditorOutcome::Close);
    assert_eq!(std::fs::read_to_string(&path).expect("reads"), "\n");

    let (mut editor, path) = message_editor(&tmp);
    press(&mut editor, "x");
    editor.on_key(&key(KeyCode::Escape));
    press(&mut editor, "ZQ");
    assert_eq!(std::fs::read_to_string(path).expect("reads"), "\n");
}

#[test]
fn test_the_first_recall_asks_git_and_lands_when_it_answers() {
    // The history is read as a job, so the window never waits on git;
    // the step back the key asked for is taken when the answer comes.
    let tmp = TempDir::new("recall-job");
    let path = tmp.0.join("COMMIT_EDITMSG");
    std::fs::write(&path, "\n# Please enter\n").expect("writes");
    let mut editor = TextEditor::new(path, None).expect("opens");
    let EditorOutcome::Job(crate::model::editor::JobRequest::Command(request)) =
        editor.recall_message(true)
    else {
        panic!("expected the history to be asked for");
    };
    assert_eq!(request.tag, git_exec::TAG_MESSAGES);
    assert_eq!(editor.buffer().lines()[0], "", "nothing swapped yet");
    editor.on_job(JobReply::Command(crate::model::editor::CommandOutput {
        id: 0,
        code: Some(0),
        stderr: String::new(),
        stdout: "earlier message\u{1e}".to_string(),
        tag: git_exec::TAG_MESSAGES,
    }));
    assert_eq!(editor.buffer().lines()[0], "earlier message");
}

#[test]
fn test_recalling_swaps_only_the_message_above_the_footer() {
    let tmp = TempDir::new("recall");
    let path = tmp.0.join("COMMIT_EDITMSG");
    std::fs::write(&path, "\n# Please enter\n# a message\n").expect("writes");
    let mut editor = TextEditor::new(path, None).expect("opens");
    editor.recall = MessageRecall::with_history(&["old", "new"]);

    editor.recall_message(true);
    assert_eq!(
        editor.buffer().lines(),
        ["new", "# Please enter", "# a message"]
    );
    editor.recall_message(true);
    assert_eq!(editor.buffer().lines()[0], "old");
    editor.recall_message(false);
    editor.recall_message(false);
    assert_eq!(
        editor.buffer().lines(),
        ["", "# Please enter", "# a message"],
        "stepping past the newest restores what was typed"
    );
}
