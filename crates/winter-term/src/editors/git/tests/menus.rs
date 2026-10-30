//! The git view's tests: the menus, and the questions they ask.

use super::*;

#[test]
fn test_every_menu_entry_runs_a_key_the_view_binds() {
    // The menu offers keys rather than commands of its own, so an entry
    // naming a chord the editor does not match does nothing when chosen.
    // The commit entry is `C-S-c`, which is exactly the shape that
    // gets written with a modifier missing.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    let over_file = editor.context_items();

    let mut header = loaded_editor();
    header.list.set_cursor(0);
    let over_header = header.context_items();

    for (items, on_file) in [(over_file, true), (over_header, false)] {
        assert!(!items.is_empty());
        for item in items.into_iter().filter(|item| !item.is_separator()) {
            let mut probe = loaded_editor();
            match on_file {
                true => cursor_on(&mut probe, "working.rs"),
                false => probe.list.set_cursor(0),
            }
            assert_ne!(
                probe.on_key(&item.keys[0]),
                EditorOutcome::Ignored,
                "the menu offers {:?}, which the view does not bind",
                item.label
            );
        }
    }
}

#[test]
fn test_the_menu_offers_each_row_what_that_row_can_do() {
    // A commit cannot be staged and a file has no hash to copy, so a menu
    // built without looking at the row under it offers both everywhere.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    let labels: Vec<String> = editor
        .context_items()
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(labels.contains(&LABEL_STAGE.to_string()));
    assert!(!labels.contains(&LABEL_COPY_HASH.to_string()));

    let commit = editor
        .rows
        .iter()
        .position(|row| matches!(row.item, Item::Commit(_)))
        .expect("the log put a commit in the view");
    editor.list.set_cursor(commit);
    let labels: Vec<String> = editor
        .context_items()
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert!(labels.contains(&LABEL_COPY_HASH.to_string()));
    assert!(!labels.contains(&LABEL_STAGE.to_string()));
}

#[test]
fn test_a_menu_key_opens_a_menu_and_hands_it_to_the_host_to_draw() {
    // The menu is the host's own key-hint card, centred over the window,
    // so the view keeps every row of the pane to itself.
    let mut editor = loaded_editor();
    editor.on_key(&press_char('Z'));
    assert_eq!(editor.menus.popup, Some(Popup::Stash));

    let hint = editor.hint().expect("a hint while the menu is open");
    assert!(
        hint.items
            .iter()
            .any(|(key, what)| key == "p" && what == "pop the one at the cursor, else the newest"),
        "got {:?}",
        hint.items
    );

    let painted = editor.content(80, 80, false);
    let text: Vec<String> = painted
        .rows
        .iter()
        .map(|row| row.iter().map(|span| span.text.clone()).collect())
        .collect();
    assert!(
        !text.iter().any(|line| line.contains("pop the newest")),
        "and nothing of it is drawn into the view, got {text:?}"
    );
}

#[test]
fn test_an_unknown_second_key_closes_the_menu_without_running_anything() {
    // A mistyped follow key must not fall through to the view's own keys,
    // where it could stage or discard something.
    let mut editor = loaded_editor();
    let outcome = choose(&mut editor, 'z', '@');
    assert_eq!(outcome, EditorOutcome::Consumed);
    assert_eq!(editor.menus.popup, None);
}

#[test]
fn test_a_branch_choice_asks_for_the_name_then_runs() {
    let mut editor = loaded_editor();
    let EditorOutcome::Prompt(request) = choose(&mut editor, 'b', 'c') else {
        panic!("expected a prompt");
    };
    let command = command_of(&editor.on_prompt(PromptReply {
        answer: Some("feature".to_string()),
        tag: request.tag,
    }));
    assert_eq!(command.args, ["checkout", "-b", "feature", "--"]);
}

#[test]
fn test_an_empty_answer_runs_nothing() {
    // `git checkout -b ""` fails with a confusing message; not running is
    // the better answer.
    let mut editor = loaded_editor();
    let EditorOutcome::Prompt(request) = choose(&mut editor, 'b', 'c') else {
        panic!("expected a prompt");
    };
    let outcome = editor.on_prompt(PromptReply {
        answer: Some("   ".to_string()),
        tag: request.tag,
    });
    assert_eq!(outcome, EditorOutcome::Consumed);
}

#[test]
fn test_a_menu_shows_however_long_the_view_is() {
    // The card is drawn over the window rather than under the rows, so a
    // status longer than the pane cannot push it off the bottom, which is
    // where a menu drawn into the view ended up.
    let mut editor = loaded_editor();
    editor.on_key(&menu_key('b'));
    let rows = 4;
    let painted = editor.content(rows, 80, false);

    assert!(editor.hint().is_some(), "the menu is there to draw");
    assert!(
        painted.rows.len() <= rows,
        "and the view still fits its pane, got {}",
        painted.rows.len()
    );
}

#[test]
fn test_switching_branch_offers_the_branches_rather_than_a_blank_line() {
    let mut editor = loaded_editor();
    let listing = choose(&mut editor, 'b', 'b');
    let command = command_of(&listing);
    assert_eq!(command.tag, git_exec::TAG_CANDIDATES);
    assert_eq!(command.args[0], "for-each-ref");
    assert!(
        command.args.iter().any(|arg| arg == "refs/remotes"),
        "a checkout takes a remote branch too, got {:?}",
        command.args
    );

    let outcome = editor.on_job(reply(
        git_exec::TAG_CANDIDATES,
        "main\nfeature\norigin/main\norigin/HEAD\n",
    ));
    let EditorOutcome::Pick(request) = outcome else {
        panic!("expected a list to pick from, got {outcome:?}");
    };
    assert_eq!(request.items, ["main", "feature", "origin/main"]);

    // Choosing one answers the question the key asked.
    let ran = editor.on_prompt(PromptReply {
        answer: Some("feature".to_string()),
        tag: request.tag,
    });
    let command = command_of(&ran);
    assert_eq!(command.args, ["checkout", "feature", "--"]);
}

#[test]
fn test_deleting_a_branch_offers_only_the_ones_that_can_be_deleted() {
    // A branch on a remote is not this repository's to delete, so the
    // list stops at the local ones.
    let mut editor = loaded_editor();
    let command = command_of(&choose(&mut editor, 'b', 'd'));
    assert!(
        command.args.iter().any(|arg| arg == "refs/heads")
            && !command.args.iter().any(|arg| arg == "refs/remotes"),
        "got {:?}",
        command.args
    );
}

#[test]
fn test_a_question_with_nothing_to_choose_from_falls_back_to_typing() {
    // A repository with no tags still has a delete-tag key; it asks for
    // one to be spelled out rather than opening an empty list.
    let mut editor = loaded_editor();
    choose(&mut editor, 't', 'd');
    let outcome = editor.on_job(reply(git_exec::TAG_CANDIDATES, "\n"));
    assert!(
        matches!(outcome, EditorOutcome::Prompt(_)),
        "expected a prompt, got {outcome:?}"
    );
}

#[test]
fn test_the_file_popup_acts_on_the_row_it_was_opened_over() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    assert_eq!(editor.on_key(&press_char('.')), EditorOutcome::Consumed);
    assert_eq!(editor.menus.popup, Some(Popup::File), "the menu is open");

    let command = command_of(&editor.on_key(&press_char('b')));
    assert_eq!(command.args[0], "blame");
    assert!(
        command.args.iter().any(|arg| arg == "working.rs"),
        "on the file the cursor was on, got {:?}",
        command.args
    );
}

#[test]
fn test_the_file_popup_stays_shut_where_there_is_no_file() {
    // On a heading or a commit there is nothing for it to act on, and a
    // menu whose every choice does nothing is worse than none.
    let mut editor = loaded_editor();
    editor.list.set_cursor(0);
    assert_eq!(editor.on_key(&press_char('.')), EditorOutcome::Consumed);
    assert_eq!(editor.menus.popup, None);
    assert_eq!(notice_text(&mut editor).as_deref(), Some("no file here"));
}

#[test]
fn test_a_search_is_answered_before_git_has_said_where_the_root_is() {
    // Every other question needs a repository and is dropped without one,
    // which would swallow a search the user typed while the status loaded.
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    search(&mut editor, "anything");
    assert_eq!(
        editor.take_notice(),
        Some(Notice::error("not found: anything"))
    );
}

#[test]
fn test_a_branch_tracking_nothing_is_behind_nothing_rather_than_failing() {
    let mut editor = loaded_editor();
    let next = editor.on_job(JobReply::Command(CommandOutput {
        id: 0,
        code: Some(128),
        stderr: "fatal: no upstream configured for branch 'main'".to_string(),
        stdout: String::new(),
        tag: git_exec::TAG_UNPULLED,
    }));
    assert_eq!(notice_text(&mut editor), None, "nothing worth reporting");
    assert!(
        matches!(next, EditorOutcome::Job(JobRequest::ReadFiles(_))),
        "the refresh carries on: {next:?}"
    );
}

#[test]
fn test_the_submodule_menu_updates_them_and_adds_one_at_a_path() {
    let mut editor = loaded_editor();
    editor.on_key(&press_char('\''));
    assert_eq!(editor.menus.popup, Some(Popup::Submodule));
    let update = command_of(&editor.on_key(&press_char('u')));
    assert_eq!(update.args, ["submodule", "update", "--init", "--recursive"]);
    editor.on_key(&press_char('\''));
    let EditorOutcome::Prompt(question) = editor.on_key(&press_char('a')) else {
        panic!("asks for the URL");
    };
    let add = command_of(&editor.on_prompt(PromptReply {
        answer: Some("https://example.com/lib.git vendor/lib".to_string()),
        tag: question.tag,
    }));
    assert_eq!(add.args[..2], ["submodule", "add"].map(String::from));
    assert!(add.args.ends_with(&[
        "https://example.com/lib.git".to_string(),
        "vendor/lib".to_string()
    ]));
}
