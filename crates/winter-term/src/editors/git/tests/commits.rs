//! The git view's tests: committing, reading a commit, and rewriting history.

use super::*;

#[test]
fn test_plus_asks_the_status_view_for_more_recent_commits() {
    // Only a list that filled its request can have more behind it; a short
    // one has shown everything there is, so the key has nothing to ask.
    let mut editor = loaded_editor();
    let before = editor.recent_count;
    let short = editor.on_key(&press_char('+'));
    assert!(
        !matches!(short, EditorOutcome::Job(_)),
        "nothing to ask for"
    );
    assert_eq!(editor.recent_count, before, "one commit is not a full list");

    // Exactly a page: the first commit is on screen, so no more.
    editor.on_job(reply(git_exec::TAG_LOG, &commits_output(before)));
    assert!(!offers_more(&editor), "the first commit is already shown");
    assert!(!matches!(
        editor.on_key(&press_char('+')),
        EditorOutcome::Job(_)
    ));

    // One past the page came back: more to load, and only a page shown.
    editor.on_job(reply(git_exec::TAG_LOG, &commits_output(before + 1)));
    assert_eq!(
        editor.repo.commits.len(),
        before,
        "the peeked commit is not shown"
    );
    assert!(offers_more(&editor));
    let more = editor.on_key(&press_char('+'));
    assert!(editor.recent_count > before, "asked for more");
    let command = command_of(&more);
    assert_eq!(command.tag, git_exec::TAG_LOG);
    assert!(
        command.args.contains(&format!(
            "--max-count={}",
            git_exec::with_peek(editor.recent_count)
        )),
        "{:?}",
        command.args
    );
}

#[test]
fn test_committing_nothing_is_refused_before_it_asks() {
    // `git commit` with an empty index fails with a wall of advice; not
    // asking for a message is the better answer.
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(reply(git_exec::TAG_STATUS, "# branch.head main\n"));
    let outcome = editor.on_key(&Key {
        alt: false,
        code: KeyCode::Char('c'),
        ctrl: true,
        shift: true,
        sup: false,
    });
    assert_eq!(outcome, EditorOutcome::Consumed);
}

#[test]
fn test_enter_in_a_commits_patch_opens_the_file_that_row_belongs_to() {
    // A commit is read as its patch, and every file band and hunk in it
    // stands for a file in the working tree: Enter did nothing there.
    let mut editor = loaded_editor();
    editor.on_job(reply(git_exec::TAG_SHOW, SHOWN_TWO_HUNKS));

    output_cursor_on(&mut editor, "src/a.rs");
    assert_eq!(
        editor.on_key(&press(KeyCode::Enter)),
        EditorOutcome::OpenPath(OpenTarget::file(PathBuf::from("/repo/src/a.rs"))),
        "the file band opens the file"
    );

    // Open the file's diff, then land on its second hunk.
    editor.on_key(&press(KeyCode::Tab));
    let output = editor.output.as_mut().expect("an output view");
    let at = output
        .rows
        .iter()
        .position(|row| row.item == Item::CommitHunk(0, 1))
        .expect("a row for the second hunk");
    output.list.set_cursor(at);
    assert_eq!(
        editor.on_key(&press(KeyCode::Enter)),
        EditorOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/src/a.rs"), 9)),
        "and a hunk opens it at the line it changes"
    );
}

#[test]
fn test_unfolding_a_commits_file_in_a_short_pane_keeps_its_row_where_it_was() {
    let mut editor = loaded_editor();
    editor.on_job(reply(git_exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    output_cursor_on(&mut editor, "src/a.rs");
    editor.content(5, 80, false);
    let scroll = editor.list.view().scroll();
    editor.on_key(&press(KeyCode::Tab));
    editor.content(5, 80, false);
    assert_eq!(
        editor.list.view().scroll(),
        scroll,
        "the opened file's row does not move"
    );
}

#[test]
fn test_the_index_or_the_working_tree_resets_alone() {
    let mut editor = loaded_editor();
    for (choice, tag, args) in [
        ('i', ASK_RESET_INDEX, vec!["reset", "main", "--", "."]),
        ('w', ASK_RESET_WORKTREE, vec!["restore", "--worktree", "--source=main", "--", "."]),
    ] {
        assert!(matches!(choose(&mut editor, 'O', choice), EditorOutcome::Prompt(_)));
        let command = command_of(&editor.on_prompt(PromptReply {
            answer: Some("main".to_string()),
            tag,
        }));
        assert_eq!(command.args, args);
    }
}

#[test]
fn test_a_switch_turned_on_in_the_commit_menu_goes_with_the_commit() {
    let mut editor = loaded_editor();
    editor.on_key(&press_char('c'));
    editor.on_key(&press_char('-'));
    editor.on_key(&press_char('a'));
    assert_eq!(editor.menus.popup, Some(Popup::Commit), "the menu stays up");
    let hint = editor.hint().expect("the menu's card");
    assert!(
        hint.items.iter().any(|(key, what)| key == "-a" && what.ends_with("[on]")),
        "{:?}",
        hint.items
    );
    let command = command_of(&editor.on_key(&press_char('c')));
    assert_eq!(command.args, ["commit", "--all"]);

    // Still on next time, until turned off.
    editor.on_key(&press_char('c'));
    editor.on_key(&press_char('-'));
    editor.on_key(&press_char('a'));
    assert_eq!(command_of(&editor.on_key(&press_char('c'))).args, ["commit"]);
}

#[test]
fn test_a_fixup_of_the_commit_at_the_cursor_and_an_instant_one_folds_it_in() {
    let mut editor = stash_editor_on(Item::Commit("abc1234".to_string()));
    let fixup = command_of(&choose(&mut editor, 'c', 'f'));
    assert_eq!(fixup.args, ["commit", "--fixup=abc1234"]);

    let instant = command_of(&choose(&mut editor, 'c', 'F'));
    assert_eq!(instant.tag, git_exec::TAG_INSTANT_FIXUP);
    let lookup = command_of(&answer_job(&mut editor, git_exec::TAG_INSTANT_FIXUP, ""));
    assert_eq!(lookup.tag, git_exec::TAG_SQUASH_FROM);
    let squash = command_of(&editor.on_job(reply(git_exec::TAG_SQUASH_FROM, "abc1234 0ff1ce5\n")));
    assert_eq!(
        squash.args,
        ["rebase", "--interactive", "--autosquash", "--autostash", "0ff1ce5"]
    );
    assert!(squash.env.iter().any(|(name, _)| name == "GIT_SEQUENCE_EDITOR"), "no plan to edit");
    // The root commit has no parent to start from: `abc1234~` failed there.
    let squash = command_of(&editor.on_job(reply(git_exec::TAG_SQUASH_FROM, "abc1234\n")));
    assert_eq!(
        squash.args,
        ["rebase", "--interactive", "--autosquash", "--autostash", "--root"]
    );
}

#[test]
fn test_a_merge_squashes_or_holds_its_commit_with_the_switches_on() {
    let mut editor = loaded_editor();
    choose(&mut editor, 'm', 's');
    assert_eq!(answer(&mut editor, ASK_MERGE_SQUASH, "topic").args, ["merge", "--squash", "topic"]);
    editor.on_key(&press_char('m'));
    editor.on_key(&press_char('-'));
    editor.on_key(&press_char('f'));
    editor.on_key(&press_char('n'));
    assert_eq!(
        answer(&mut editor, ASK_MERGE_NO_COMMIT, "topic").args,
        ["merge", "--no-commit", "--ff-only", "topic"]
    );
}

#[test]
fn test_the_diff_menu_reads_a_branch_against_another_and_a_commit() {
    let mut editor = loaded_editor();
    choose(&mut editor, 'd', 'b');
    assert_eq!(answer(&mut editor, ASK_DIFF_BRANCH, "main").args, ["diff", "--no-color", "main...HEAD", "--"]);
    choose(&mut editor, 'd', 'c');
    let commit = editor.on_prompt(PromptReply {
        answer: Some("abc1234".to_string()),
        tag: ASK_SHOW_COMMIT,
    });
    assert!(matches!(commit, EditorOutcome::GitView(_)), "in a tab of its own: {commit:?}");
}

#[test]
fn test_ctrl_shift_c_commits_in_the_editor_when_something_is_staged() {
    let mut editor = loaded_editor();
    let outcome = editor.on_key(&Key {
        alt: false,
        code: KeyCode::Char('c'),
        ctrl: true,
        shift: true,
        sup: false,
    });
    let request = command_of(&outcome);
    assert_eq!(request.tag, git_exec::TAG_COMMIT);
    assert_eq!(request.args, ["commit"]);
}

#[test]
fn test_a_commit_runs_headless_with_winters_editor_and_no_terminal_tab() {
    // The editor is a tab of this window that git waits on, so the commit
    // needs no terminal of its own, and a spawn would leave an empty one
    // sitting beside the message.
    let mut editor = loaded_editor();
    let outcome = choose(&mut editor, 'c', 'c');
    assert!(matches!(outcome, EditorOutcome::Job(_)), "{outcome:?}");
    let request = command_of(&outcome);
    assert_eq!(request.args, ["commit"]);
    assert_eq!(request.cwd, PathBuf::from("/repo"));
    assert!(
        request.env.iter().any(|(name, _)| name == "GIT_EDITOR"),
        "git is pointed at Winter's editor: {:?}",
        request.env
    );
    let amend = command_of(&choose(&mut editor, 'c', 'a'));
    assert_eq!(amend.args, ["commit", "--amend"]);
}

#[test]
fn test_an_interactive_rebase_also_goes_to_a_pane() {
    let mut editor = loaded_editor();
    // The branch to rebase onto is chosen from a list, so the key asks
    // for the names first and the answer arrives under the same tag.
    let listing = choose(&mut editor, 'r', 'i');
    assert_eq!(command_of(&listing).tag, git_exec::TAG_CANDIDATES);
    let EditorOutcome::Pick(request) =
        editor.on_job(reply(git_exec::TAG_CANDIDATES, "main\nfeature\n"))
    else {
        panic!("expected a list to pick from");
    };
    let outcome = editor.on_prompt(PromptReply {
        answer: Some("HEAD~3".to_string()),
        tag: request.tag,
    });
    let EditorOutcome::Spawn(spawn) = outcome else {
        panic!("expected a spawn, got {outcome:?}");
    };
    assert_eq!(spawn.args, ["rebase", "--interactive", "HEAD~3"]);
}

#[test]
fn test_enter_on_a_commit_reads_it() {
    // Enter opens the file under the cursor, and a commit row has no file:
    // it used to be swallowed, so a commit was the one row Enter did
    // nothing on.
    let mut editor = loaded_editor();
    let at = editor
        .rows
        .iter()
        .position(|row| matches!(row.item, Item::Commit(_)))
        .expect("a commit row");
    editor.list.set_cursor(at);
    let command = command_of(&editor.on_key(&press(KeyCode::Enter)));
    assert_eq!(command.tag, git_exec::TAG_SHOW);
    assert_eq!(command.args.first().map(String::as_str), Some("show"));
    assert!(command
        .args
        .ends_with(&["abc1234".to_string(), "--".to_string()]));
    assert!(
        !command.args.iter().any(|arg| arg == "--shortstat"),
        "no line totals: the view counts the commit's own hunks, {:?}",
        command.args
    );
}

#[test]
fn test_a_commit_read_fills_the_view_under_its_own_title() {
    let mut editor = loaded_editor();
    // Joined rather than written as one literal: `git show` output is
    // indentation-sensitive (the subject is the message block's four-space
    // indent), and a continued string literal cannot carry that faithfully.
    let shown = [
        "commit abc1234567890",
        "Author: Someone <a@b.c>",
        "Date:   2026-09-15 12:00:00 +0000",
        "",
        "    do the thing",
        "",
        " src/a.rs | 2 +-",
    ]
    .join("\n");
    assert_eq!(
        editor.on_job(reply(git_exec::TAG_SHOW, &shown)),
        EditorOutcome::Consumed
    );
    let output = editor
        .output
        .as_ref()
        .expect("the commit replaced the status view");
    assert_eq!(output.title, "Commit abc12345  do the thing");
    assert!(output.lines.iter().any(|line| line.contains("src/a.rs")));
}

#[test]
fn test_a_commit_content_view_bands_its_files() {
    // Enter on a commit shows its content the way the working tree reads:
    // the summary receded, then a band per file and decorated hunks
    // under it, no `diff --git` or `index` lines.
    let shown = [
        "commit abc1234567890",
        "Author: Someone <a@b.c>",
        "Date:   2026-09-15 12:00:00 +0000",
        "",
        "    do the thing",
        "",
        "diff --git a/src/a.rs b/src/a.rs",
        "index 111..222 100644",
        "--- a/src/a.rs",
        "+++ b/src/a.rs",
        "@@ -1,2 +1,2 @@",
        " fn main() {",
        "-    old();",
        "+    new();",
    ]
    .join("\n");
    let mut editor = loaded_editor();
    assert_eq!(
        editor.on_job(reply(git_exec::TAG_SHOW, &shown)),
        EditorOutcome::Consumed
    );
    // A commit opens shut, so open all of it to read what it paints.
    editor.on_key(&shift(KeyCode::Tab));
    let output = editor
        .output
        .as_ref()
        .expect("the commit replaced the status view");
    assert!(
        output
            .rows
            .iter()
            .any(|row| row_text(&row.spans).contains("src/a.rs")),
        "the file band"
    );
    assert!(
        output
            .rows
            .iter()
            .all(|row| !row_text(&row.spans).contains("diff --git")),
        "the patch's plumbing lines are gone"
    );
    let added = output
        .rows
        .iter()
        .find(|row| row_text(&row.spans).contains("new();"))
        .expect("the added line");
    assert!(added
        .spans
        .iter()
        .any(|span| span.style == Style::AddedEdit));
    assert_eq!(
        output.lines.len(),
        output.rows.len(),
        "the search text matches the painted rows"
    );
}

#[test]
fn test_tab_on_a_commits_heading_shuts_every_file_under_it() {
    // The heading is the commit's own section, the way the working tree's
    // sections are: one key takes the whole file list away and brings it
    // back, with the count left saying what is behind it.
    let mut editor = loaded_editor();
    editor.on_job(reply(git_exec::TAG_SHOW, SHOWN_TWO_FILES));
    assert!(
        output_has(&editor, "src/a.rs"),
        "the files show to begin with"
    );

    output_cursor_on(&mut editor, "Changes");
    assert_eq!(editor.on_key(&press(KeyCode::Tab)), EditorOutcome::Consumed);
    assert!(
        !output_has(&editor, "src/a.rs") && !output_has(&editor, "src/b.rs"),
        "the file list is away"
    );
    assert!(
        output_has(&editor, "Changes (2)"),
        "the heading stays, still counting what it holds"
    );

    editor.on_key(&press(KeyCode::Tab));
    assert!(
        output_has(&editor, "src/a.rs") && output_has(&editor, "src/b.rs"),
        "and comes back"
    );
}

#[test]
fn test_a_commit_opens_shut_and_tab_opens_it_one_layer_at_a_time() {
    let mut editor = loaded_editor();
    editor.on_job(reply(git_exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    assert!(
        output_has(&editor, "src/a.rs"),
        "the commit opens as the files it touched"
    );
    assert!(
        !output_has(&editor, "@@") && !output_has(&editor, "new();"),
        "with neither their hunks nor their diffs"
    );

    output_cursor_on(&mut editor, "src/a.rs");
    let at = editor.output.as_ref().unwrap().list.cursor();
    assert_eq!(editor.on_key(&press(KeyCode::Tab)), EditorOutcome::Consumed);
    assert!(
        output_has(&editor, "@@"),
        "the file opens to its hunk headers"
    );
    assert!(
        !output_has(&editor, "new();"),
        "which are themselves still shut"
    );
    assert_eq!(
        editor.output.as_ref().unwrap().list.cursor(),
        at,
        "the cursor stays on the band it acted on"
    );

    output_cursor_on(&mut editor, "@@ -1,2 +1,2 @@");
    editor.on_key(&press(KeyCode::Tab));
    assert!(output_has(&editor, "new();"), "the hunk opens to its body");

    output_cursor_on(&mut editor, "src/a.rs");
    editor.on_key(&press(KeyCode::Tab));
    assert!(
        !output_has(&editor, "@@"),
        "the file shuts all of it away again"
    );
    assert!(
        output_has(&editor, "src/a.rs"),
        "its band stays, so it can be opened again"
    );
}

#[test]
fn test_tab_on_a_commits_hunk_folds_only_that_hunk() {
    let mut editor = loaded_editor();
    editor.on_job(reply(git_exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    editor.on_key(&shift(KeyCode::Tab));

    output_cursor_on(&mut editor, "@@ -1,2 +1,2 @@");
    editor.on_key(&press(KeyCode::Tab));

    assert!(
        output_has(&editor, "@@ -1,2 +1,2 @@"),
        "a folded hunk keeps its header"
    );
    assert!(!output_has(&editor, "new();"), "but drops its body");
    assert!(
        output_has(&editor, "third();"),
        "the other hunk is untouched"
    );
}

#[test]
fn test_tab_on_a_commits_summary_does_nothing() {
    // The summary has nothing to fold, so Tab must not disturb the view.
    let mut editor = loaded_editor();
    editor.on_job(reply(git_exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    output_cursor_on(&mut editor, "do the thing");
    let before = editor.output.as_ref().unwrap().lines.clone();

    assert_eq!(editor.on_key(&press(KeyCode::Tab)), EditorOutcome::Consumed);
    assert_eq!(
        editor.output.as_ref().unwrap().lines,
        before,
        "the view is unchanged"
    );
}

#[test]
fn test_shift_tab_folds_every_file_of_a_commit_and_opens_them_again() {
    let mut editor = loaded_editor();
    editor.on_job(reply(git_exec::TAG_SHOW, SHOWN_TWO_FILES));
    assert!(
        !output_has(&editor, "new();") && !output_has(&editor, "third();"),
        "the commit opens shut"
    );

    assert_eq!(editor.on_key(&shift(KeyCode::Tab)), EditorOutcome::Consumed);
    assert!(
        output_has(&editor, "new();") && output_has(&editor, "third();"),
        "one key opens every file and hunk of it"
    );

    editor.on_key(&shift(KeyCode::Tab));
    assert!(
        !output_has(&editor, "new();") && !output_has(&editor, "third();"),
        "a second S-TAB shuts all of it again"
    );
    let output = editor.output.as_ref().expect("the commit");
    assert_eq!(
        output.folded_hunks.len(),
        2,
        "and every hunk with it, so a file opened again shows only their headers"
    );
    assert!(
        output_has(&editor, "src/a.rs") && output_has(&editor, "src/b.rs"),
        "the bands stay, so the commit still says what it touched"
    );
}

#[test]
fn test_shift_tab_opens_a_part_way_folded_commit_rather_than_shutting_it() {
    // One hunk folded by hand means the view is already part-way shut, so
    // the key that acts on all of it opens it rather than folding further.
    let mut editor = loaded_editor();
    editor.on_job(reply(git_exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    editor.on_key(&shift(KeyCode::Tab));
    output_cursor_on(&mut editor, "@@ -1,2 +1,2 @@");
    editor.on_key(&press(KeyCode::Tab));
    assert!(!output_has(&editor, "new();"), "that hunk's body is away");

    editor.on_key(&shift(KeyCode::Tab));
    assert!(
        output_has(&editor, "new();") && output_has(&editor, "third();"),
        "every hunk is back"
    );
}

#[test]
fn test_a_commits_file_band_carries_an_icon() {
    let mut editor = loaded_editor();
    editor.on_job(reply(git_exec::TAG_SHOW, SHOWN_TWO_HUNKS));
    let output = editor.output.as_ref().expect("the commit view");
    let band = output
        .rows
        .iter()
        .find(|row| row_text(&row.spans).contains("src/a.rs"))
        .expect("the file band");
    assert_eq!(
        band.icon.as_ref().map(|icon| &icon.kind),
        Some(&crate::model::paint::IconKind::File {
            name: "a.rs".to_string()
        }),
        "the band names its icon for the file's leaf"
    );
    assert!(
        output.rows.iter().filter(|row| row.icon.is_some()).count() == 1,
        "only the file band carries one; hunk lines do not"
    );
}

#[test]
fn test_an_empty_commit_read_says_so_instead_of_blanking_the_view() {
    let mut editor = loaded_editor();
    editor.on_job(reply(
        git_exec::TAG_SHOW,
        "   
",
    ));
    assert!(editor.output.is_none(), "the status view stays");
    assert_eq!(notice_text(&mut editor).as_deref(), Some("nothing to show"));
}

#[test]
fn test_reset_on_a_commit_uses_that_commit_without_asking() {
    let mut editor = loaded_editor();
    let at = editor
        .rows
        .iter()
        .position(|row| matches!(row.item, Item::Commit(_)))
        .expect("a commit row");
    editor.list.set_cursor(at);
    let command = command_of(&editor.on_key(&press_char('o')));
    assert_eq!(command.args, ["reset", "--mixed", "abc1234", "--"]);
}

#[test]
fn test_reset_away_from_a_commit_asks_which_one() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    let EditorOutcome::Prompt(request) = editor.on_key(&press_char('o')) else {
        panic!("expected a prompt");
    };
    assert_eq!(request.tag, ASK_RESET_MIXED);
}

#[test]
fn test_a_hard_reset_says_what_it_costs() {
    let mut editor = loaded_editor();
    let EditorOutcome::Prompt(request) = choose(&mut editor, 'O', 'h') else {
        panic!("expected a prompt");
    };
    assert!(request.label.contains("HARD"), "got {:?}", request.label);
}

#[test]
fn test_yanking_copies_the_hash_on_a_commit_and_the_path_on_a_file() {
    let mut editor = loaded_editor();
    let at = editor
        .rows
        .iter()
        .position(|row| matches!(row.item, Item::Commit(_)))
        .expect("a commit row");
    editor.list.set_cursor(at);
    editor.on_key(&press_char('y'));
    assert_eq!(
        editor.on_key(&press_char('s')),
        EditorOutcome::Yank("abc1234".to_string())
    );

    cursor_on(&mut editor, "working.rs");
    editor.on_key(&press_char('y'));
    assert_eq!(
        editor.on_key(&press_char('s')),
        EditorOutcome::Yank("working.rs".to_string())
    );
    editor.on_key(&press_char('y'));
    assert!(
        matches!(editor.on_key(&press_char('y')), EditorOutcome::Yank(line) if line.contains("working.rs")),
        "yy copies the line as it reads"
    );
}

#[test]
fn test_a_branch_with_no_commits_yet_reads_as_an_empty_history_not_a_failure() {
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(reply(git_exec::TAG_STATUS, STATUS_OUTPUT));
    let next = editor.on_job(JobReply::Command(CommandOutput {
        id: 0,
        code: Some(128),
        stderr: "fatal: your current branch 'main' does not have any commits yet\n".to_string(),
        stdout: String::new(),
        tag: git_exec::TAG_LOG,
    }));
    assert!(editor.notice.is_none(), "no failure said: {:?}", editor.notice);
    assert!(editor.repo.commits.is_empty());
    assert_eq!(command_of(&next).tag, git_exec::TAG_STASHES, "the chain carries on");
}
