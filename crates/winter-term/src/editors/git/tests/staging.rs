//! The git view's tests: staging, unstaging, discarding, and resolving conflicts.

use super::*;

#[test]
fn test_resolving_a_conflict_takes_one_side_then_stages_the_file() {
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(reply(
        git_exec::TAG_STATUS,
        "# branch.head main\nu UU N... 100644 100644 100644 100644 a b c fought.rs\n",
    ));
    cursor_on(&mut editor, "fought.rs");
    editor.on_key(&press(KeyCode::Char('.')));
    let command = command_of(&editor.on_key(&press(KeyCode::Char('t'))));
    assert_eq!(command.args, ["checkout", "--theirs", "--", "fought.rs"]);
    let next = command_of(&answer_job(&mut editor, git_exec::TAG_RESOLVE, ""));
    assert_eq!(
        next.args,
        ["add", "--", "fought.rs"],
        "staging marks it resolved"
    );
}

#[test]
fn test_two_conflicts_resolved_at_once_each_stage_their_own_file() {
    // One slot for the path being resolved: the second conflict had to wait
    // for the first, and before that, its answer staged whichever path the
    // slot held. Answered out of order, each stages its own.
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(reply(
        git_exec::TAG_STATUS,
        "# branch.head main\nu UU N... 100644 100644 100644 100644 a b c one.rs\nu UU N... 100644 100644 100644 100644 a b c two.rs\n",
    ));
    let take_theirs = |editor: &mut GitEditor, path: &str| {
        cursor_on(editor, path);
        editor.on_key(&press(KeyCode::Char('.')));
        command_of(&editor.on_key(&press(KeyCode::Char('t'))))
    };
    let one = take_theirs(&mut editor, "one.rs");
    let two = take_theirs(&mut editor, "two.rs");
    let answered = |editor: &mut GitEditor, asked: &CommandRequest| {
        command_of(&editor.on_job(JobReply::Command(CommandOutput {
            code: Some(0),
            id: asked.id,
            tag: asked.tag,
            ..CommandOutput::default()
        })))
        .args
    };
    assert_eq!(answered(&mut editor, &two), ["add", "--", "two.rs"]);
    assert_eq!(answered(&mut editor, &one), ["add", "--", "one.rs"]);
}

#[test]
fn test_staging_acts_on_the_path_under_the_cursor() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    let command = command_of(&editor.on_key(&press(KeyCode::Char('s'))));
    assert_eq!(command.args, ["add", "--", "working.rs"]);
}

#[test]
fn test_staging_on_a_heading_acts_on_that_whole_section() {
    // The heading stands for its section, which is what makes "stage this
    // lot" one keystroke.
    let mut editor = loaded_editor();
    let at = editor
        .rows
        .iter()
        .position(|row| row.item == Item::Heading(Section::Unstaged))
        .expect("the unstaged heading");
    editor.list.set_cursor(at);
    let command = command_of(&editor.on_key(&press(KeyCode::Char('s'))));
    assert_eq!(command.args, ["add", "--", "working.rs"]);
}

#[test]
fn test_stage_all_from_the_untracked_section_stages_only_untracked_files() {
    // Magic's stage-all splits by section: from untracked it stages the
    // files that section lists, by name.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "new.rs");
    let command = command_of(&editor.on_key(&ctrl_shift('s')));
    assert_eq!(command.args, ["add", "--", "new.rs"]);
}

#[test]
fn test_stage_all_elsewhere_stages_tracked_changes_only() {
    // Anywhere else it is `add -u`, so an untracked file stays that way
    // until asked for by name.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    let command = command_of(&editor.on_key(&ctrl_shift('s')));
    assert_eq!(command.args, ["add", "-u"]);
}

#[test]
fn test_stage_untracked_stages_every_untracked_file_from_anywhere() {
    // Unlike stage-all, the cursor's section does not matter: from a tracked
    // change it still stages the untracked files, and only those.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    let command = command_of(&editor.on_key(&ctrl_shift('a')));
    assert_eq!(command.args, ["add", "--", "new.rs"]);
}

#[test]
fn test_unstaging_uses_the_staged_copy_of_a_file_changed_on_both_sides() {
    // `staged.rs` and `working.rs` differ, but a file in both sections must
    // unstage from the staged row and stage from the unstaged one.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "staged.rs");
    let command = command_of(&editor.on_key(&press(KeyCode::Char('u'))));
    assert_eq!(command.args, ["restore", "--staged", "--", "staged.rs"]);
}

#[test]
fn test_discarding_asks_first_and_names_the_file() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    let EditorOutcome::Prompt(request) = editor.on_key(&press(KeyCode::Char('x'))) else {
        panic!("expected a confirmation");
    };
    assert_eq!(request.mode, PromptMode::Confirm);
    assert!(
        request.label.contains("working.rs"),
        "got {:?}",
        request.label
    );
}

#[test]
fn test_discarding_in_the_staged_section_is_refused() {
    // `restore --worktree` on a staged file threw away its unstaged
    // changes, which were not the ones on screen.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "staged.rs");
    assert_eq!(editor.on_key(&press(KeyCode::Char('x'))), EditorOutcome::Consumed);
    assert!(editor.menus.pending_discard.is_none());
}

#[test]
fn test_discarding_an_untracked_file_deletes_it_instead_of_restoring() {
    // `restore` cannot reach a path that is not in the index, so the view
    // would report success while leaving the file exactly where it was.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "new.rs");
    editor.on_key(&press(KeyCode::Char('x')));
    let command = command_of(&editor.on_prompt(PromptReply {
        answer: Some("y".to_string()),
        tag: ASK_DISCARD,
    }));
    assert_eq!(command.args, ["clean", "--force", "-d", "--", "new.rs"]);
}

#[test]
fn test_declining_the_confirmation_runs_nothing() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    editor.on_key(&press(KeyCode::Char('x')));
    let outcome = editor.on_prompt(PromptReply {
        answer: None,
        tag: ASK_DISCARD,
    });
    assert_eq!(outcome, EditorOutcome::Consumed);
}

#[test]
fn test_enter_on_a_hunk_opens_its_file_at_the_line_the_hunk_changes() {
    // Opening the file at line one from a hunk throws away the one thing
    // the cursor's position said: which part of it is being read.
    let mut editor = editor_with_diff();
    cursor_on_hunk(&mut editor, 1);
    assert_eq!(
        editor.on_key(&press(KeyCode::Enter)),
        EditorOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/working.rs"), 10))
    );

    // Regression: a line inside the hunk opened at the hunk's start, not at
    // the line under the cursor. Two rows below the header is `+ added()`,
    // the hunk's second line on the new side.
    let header = editor.list.cursor();
    editor.list.set_cursor(header + 2);
    assert_eq!(
        editor.on_key(&press(KeyCode::Enter)),
        EditorOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/working.rs"), 11))
    );

    // And the file's own band still opens it at the top, since it names
    // no line in particular.
    cursor_on(&mut editor, "working.rs");
    assert_eq!(
        editor.on_key(&press(KeyCode::Enter)),
        EditorOutcome::OpenPath(OpenTarget::file(PathBuf::from("/repo/working.rs")))
    );
}

#[test]
fn test_an_expanded_file_lists_its_hunks_and_lines() {
    let editor = editor_with_diff();
    let hunks: Vec<usize> = editor
        .rows
        .iter()
        .filter_map(|row| match &row.item {
            Item::Hunk(hunk) => Some(hunk.index),
            _ => None,
        })
        .collect();
    assert!(hunks.contains(&0) && hunks.contains(&1), "got {hunks:?}");
    assert!(hunks.len() > 2, "the lines belong to their hunk too");
}

#[test]
fn test_staging_on_a_hunk_applies_that_hunk_alone() {
    // The whole point of hunk staging: the second hunk must not be in the
    // patch, and the patch goes in on standard input.
    let mut editor = editor_with_diff();
    cursor_on_hunk(&mut editor, 0);
    let command = command_of(&editor.on_key(&press(KeyCode::Char('s'))));
    assert_eq!(command.args, ["apply", "--cached", "-"]);
    let patch = command.stdin.clone().expect("the patch");
    assert!(patch.contains("-    old();"), "got {patch}");
    assert!(!patch.contains("added()"), "got {patch}");
}

#[test]
fn test_a_selection_over_one_line_of_a_hunk_stages_that_line_alone() {
    let mut editor = editor_with_diff();
    cursor_on_hunk(&mut editor, 0);
    // Header, ` fn main() {`, then `-    old();`.
    editor.list.move_by(2);
    let at = editor.list.cursor();
    let outcome = editor.on_selected_rows_key(&press_char('s'), at..=at).expect("taken");
    let command = command_of(&outcome);
    let patch = command.stdin.clone().expect("the patch");
    assert!(patch.contains("@@ -1,2 +1,1 @@"), "recounted: {patch}");
    assert!(
        patch.contains("\n-    old();\n"),
        "the chosen removal: {patch}"
    );
    assert!(
        !patch.contains("new();"),
        "the unchosen addition is dropped: {patch}"
    );
}

#[test]
fn test_a_selection_over_lines_of_a_hunk_stages_the_lines_it_covers() {
    let mut editor = editor_with_diff();
    cursor_on_hunk(&mut editor, 0);
    editor.list.move_by(2);
    let from = editor.list.cursor();
    let outcome = editor.on_selected_rows_key(&press_char('s'), from..=from + 1).expect("taken");
    let patch = command_of(&outcome).stdin.expect("the patch");
    assert!(
        patch.contains("-    old();\n+    new();"),
        "both lines: {patch}"
    );
}

#[test]
fn test_a_selection_over_files_stages_unstages_or_discards_them_all() {
    let mut editor = loaded_editor();
    let (staged, working, new) = (row_of(&editor, "staged.rs"), row_of(&editor, "working.rs"), row_of(&editor, "new.rs"));
    let all = staged.min(working).min(new)..=staged.max(working).max(new);
    let stage = editor.on_selected_rows_key(&press_char('s'), all.clone()).expect("taken");
    assert_eq!(command_of(&stage).args, ["add", "--", "new.rs", "working.rs"], "the staged one left out");
    let unstage = editor.on_selected_rows_key(&press_char('u'), all.clone()).expect("taken");
    assert_eq!(command_of(&unstage).args, ["restore", "--staged", "--", "staged.rs"]);
    assert!(
        matches!(editor.on_selected_rows_key(&press_char('x'), working..=working), Some(EditorOutcome::Prompt(_))),
        "asks before discarding"
    );
    assert_eq!(editor.on_selected_rows_key(&press_char('j'), all), None, "a motion stays the selection's");
}

#[test]
fn test_a_selection_inside_a_hunk_stages_just_its_lines() {
    let mut editor = editor_with_diff();
    cursor_on_hunk(&mut editor, 0);
    let header = editor.list.cursor();
    let (from, to) = (header + 2, header + 3);
    let outcome = editor.on_selected_rows_key(&press_char('s'), from..=to).expect("taken");
    let patch = command_of(&outcome).stdin.expect("the patch");
    assert!(patch.contains("-    old();\n+    new();"), "both lines: {patch}");
}

#[test]
fn test_a_key_inside_a_hunk_acts_on_that_hunk() {
    // Only the header row would be a cruel target; every line of the hunk
    // stands for it.
    let mut editor = editor_with_diff();
    let line = editor
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| matches!(&row.item, Item::Hunk(hunk) if hunk.index == 1))
        .nth(1)
        .map(|(index, _)| index)
        .expect("a line inside the second hunk");
    editor.list.set_cursor(line);
    let command = command_of(&editor.on_key(&press(KeyCode::Char('s'))));
    let patch = command.stdin.clone().expect("the patch");
    assert!(patch.contains("+    added();"), "got {patch}");
}

#[test]
fn test_unstaging_a_hunk_reverses_it_out_of_the_index() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "staged.rs");
    editor.on_key(&press(KeyCode::Tab));
    answer_job(&mut editor, git_exec::TAG_DIFF, DIFF_OUTPUT);
    cursor_on_hunk(&mut editor, 0);
    let command = command_of(&editor.on_key(&press(KeyCode::Char('u'))));
    assert_eq!(
        command.args,
        ["apply", "--cached", "--reverse", "-"]
    );

    // From the unstaged side the patch is not the index's to take back.
    let mut editor = editor_with_diff();
    cursor_on_hunk(&mut editor, 0);
    assert_eq!(editor.on_key(&press(KeyCode::Char('u'))), EditorOutcome::Consumed);
}

#[test]
fn test_discarding_a_hunk_reverses_it_out_of_the_working_tree() {
    let mut editor = editor_with_diff();
    cursor_on_hunk(&mut editor, 0);
    editor.on_key(&press(KeyCode::Char('x')));
    let command = command_of(&editor.on_prompt(PromptReply {
        answer: Some("y".to_string()),
        tag: ASK_DISCARD,
    }));
    assert_eq!(command.args, ["apply", "--reverse", "-"]);
}

#[test]
fn test_ctrl_shift_c_with_nothing_staged_says_so_rather_than_doing_nothing() {
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(reply(git_exec::TAG_STATUS, "# branch.head main\n? new.rs\n"));
    let outcome = editor.on_key(&Key {
        alt: false,
        code: KeyCode::Char('c'),
        ctrl: true,
        shift: true,
        sup: false,
    });
    assert_eq!(outcome, EditorOutcome::Consumed);
    assert_eq!(
        notice_text(&mut editor).as_deref(),
        Some("nothing staged to commit")
    );
}

#[test]
fn test_ignoring_by_extension_writes_a_wildcard() {
    let tree = std::env::temp_dir().join(format!("winter-gitignore-{}", std::process::id()));
    std::fs::create_dir_all(&tree).expect("temp dir");
    let mut editor = GitEditor::new(tree.clone());
    editor.on_job(reply(git_exec::TAG_ROOT, &format!("{}\n", tree.display())));
    editor.on_job(reply(git_exec::TAG_STATUS, STATUS_OUTPUT));
    cursor_on(&mut editor, "working.rs");

    choose(&mut editor, 'i', 'e');
    let written = std::fs::read_to_string(tree.join(".gitignore")).unwrap_or_default();
    assert_eq!(written.trim(), "*.rs");
    std::fs::remove_dir_all(&tree).ok();
}

#[test]
fn test_the_keymaps_apply_keys_act_on_a_hunk_only() {
    // `a` and `-` are hunk keys; on a file row there is no patch to apply,
    // and guessing one would stage the whole file by surprise.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    assert_eq!(editor.on_key(&press_char('a')), EditorOutcome::Consumed);
    assert_eq!(editor.on_key(&press_char('-')), EditorOutcome::Consumed);
}

#[test]
fn test_the_file_menu_asks_before_discarding() {
    // `.` then `x` throws away as much as `x` does, so it asks the same.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    let outcome = choose(&mut editor, '.', 'x');
    let EditorOutcome::Prompt(request) = outcome else {
        panic!("expected a confirmation, got {outcome:?}");
    };
    assert_eq!(request.mode, PromptMode::Confirm);
    assert!(request.label.contains("working.rs"), "{:?}", request.label);
}

#[test]
fn test_a_confirmed_discard_acts_on_what_the_question_named() {
    // The tree is read again while the question is open, and a file
    // appearing above moves every row down one: the yes is still about
    // the file it was asked about.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    let EditorOutcome::Prompt(request) = editor.on_key(&press_char('x')) else {
        panic!("expected a confirmation");
    };
    let moved = STATUS_OUTPUT.replace(
        "1 .M N... 100644 100644 100644 aaa bbb working.rs",
        "1 .M N... 100644 100644 100644 aaa bbb aaa-first.rs\n\
         1 .M N... 100644 100644 100644 aaa bbb working.rs",
    );
    editor.on_job(reply(git_exec::TAG_STATUS, &moved));
    let command = command_of(&editor.on_prompt(yes(request.tag)));
    assert_eq!(command.args, ["restore", "--worktree", "--", "working.rs"]);
}

#[test]
fn test_a_discard_turned_down_is_forgotten() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    let EditorOutcome::Prompt(request) = editor.on_key(&press_char('x')) else {
        panic!("expected a confirmation");
    };
    editor.on_prompt(PromptReply {
        answer: None,
        tag: request.tag,
    });
    assert_eq!(
        editor.on_prompt(yes(request.tag)),
        EditorOutcome::Consumed,
        "a stray yes later discards nothing"
    );
}

#[test]
fn test_an_ignored_path_is_anchored_and_escaped() {
    assert_eq!(ignore_pattern("out.log", false), "/out.log");
    assert_eq!(ignore_pattern("a/[x]*.txt", false), "/a/\\[x]\\*.txt");
    assert_eq!(ignore_pattern("trailing ", false), "/trailing\\ ");
    assert_eq!(ignore_pattern("src/main.rs", true), "*.rs");
    // A dot in a directory is no extension of the file under it.
    assert_eq!(ignore_pattern("a.d/Makefile", true), "/a.d/Makefile");
}
