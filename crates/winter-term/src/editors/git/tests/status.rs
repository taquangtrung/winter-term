//! The git view's tests: reading the status, and what it reports.

use super::*;

#[test]
fn test_tag_root_failure_shows_error_instead_of_loading() {
    let mut editor = GitEditor::new(PathBuf::from("/not-a-repo"));
    editor.on_job(JobReply::Command(CommandOutput {
        id: 0,
        code: Some(128),
        stderr: "fatal: not a git repository (or any of the parent directories): .git".to_string(),
        stdout: String::new(),
        tag: git_exec::TAG_ROOT,
    }));
    let text = painted_text(&mut editor);
    assert!(text.contains(NOT_A_REPO), "{text}");
    assert!(!text.contains("fatal"), "git's words go to the status bar");
    let notice = editor.take_notice().expect("a failure is reported");
    assert_eq!(notice.severity, Severity::Error);
    assert!(
        notice
            .text
            .starts_with("failed: fatal: not a git repository"),
        "{}",
        notice.text
    );
}

#[test]
fn test_a_status_that_times_out_shows_the_failure_not_the_loading_note() {
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(JobReply::Command(CommandOutput {
        id: 0,
        code: None,
        stderr: "command timed out after 15s".to_string(),
        stdout: String::new(),
        tag: git_exec::TAG_STATUS,
    }));
    let text = painted_text(&mut editor);
    assert!(text.contains(UNREADABLE), "{text}");
    assert!(!text.contains(LOADING));
    let notice = notice_text(&mut editor).unwrap_or_default();
    assert!(notice.contains("command timed out after 15s"), "{notice}");
}

#[test]
fn test_a_report_goes_to_the_status_bar_not_the_header() {
    // The header is the repository's state; a report drawn there pushed
    // the sections down a row and stayed until the next key.
    let mut editor = loaded_editor();
    editor.on_job(reply(git_exec::TAG_FETCH, ""));
    assert!(!painted_text(&mut editor).contains("fetched"));
    let notice = editor.take_notice().expect("the fetch is reported");
    assert_eq!(notice, Notice::info("fetched"));
    assert_eq!(editor.take_notice(), None, "a report is shown once");
}

#[test]
fn test_the_root_lookup_leads_to_a_status_read() {
    // Every later command runs from the root, so the view cannot do
    // anything until that answer arrives.
    let mut editor = GitEditor::new(PathBuf::from("/repo/sub"));
    let next = editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    let command = command_of(&next);
    assert_eq!(command.tag, git_exec::TAG_STATUS);
    assert_eq!(command.cwd, PathBuf::from("/repo"), "not the subdirectory");
}

#[test]
fn test_a_command_that_changes_the_repository_is_named_as_it_starts() {
    // The status bar says what a key ran; a read the view makes on its own
    // says nothing, and the name stays out of the notification history.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "working.rs");
    editor.on_key(&press(KeyCode::Char('s')));
    let notice = editor.take_notice().expect("the command is named");
    assert_eq!(notice.text, "git add working.rs");
    assert!(!notice.logged);
    editor.on_job(reply(git_exec::TAG_STAGE, ""));
    assert_eq!(editor.take_notice(), None, "the re-read is not named");
}

#[test]
fn test_a_command_that_changed_the_tree_re_reads_it() {
    // Without the re-read the view shows the state before the change, and
    // the next key acts on rows that no longer describe the repository.
    let mut editor = loaded_editor();
    let next = editor.on_job(reply(git_exec::TAG_STAGE, ""));
    assert_eq!(command_of(&next).tag, git_exec::TAG_STATUS);
    assert_eq!(
        editor.take_notice(),
        None,
        "the section headings already say it"
    );
}

#[test]
fn test_a_failed_command_reports_and_does_not_re_read() {
    let mut editor = loaded_editor();
    let outcome = editor.on_job(JobReply::Command(CommandOutput {
        id: 0,
        code: Some(1),
        stderr: "error: pathspec 'nope' did not match\n".to_string(),
        stdout: String::new(),
        tag: git_exec::TAG_STAGE,
    }));
    assert_eq!(outcome, EditorOutcome::Consumed);
    let message = notice_text(&mut editor).unwrap_or_default();
    assert!(message.starts_with("failed:"), "got {message:?}");
    assert!(message.contains("pathspec"), "got {message:?}");
}

#[test]
fn test_opening_a_file_already_showing_in_full_leaves_the_editor_alone() {
    // The scroll only moves for a block that runs past the pane's bottom:
    // one that fits must not shift the editor under the reader.
    let mut editor = editor_with_diff();
    editor.content(40, 80, false);
    let before = editor.list.view().scroll();
    editor.on_key(&press(KeyCode::Tab));
    editor.content(40, 80, false);
    editor.on_key(&press(KeyCode::Tab));
    editor.content(40, 80, false);
    assert_eq!(
        editor.list.view().scroll(),
        before,
        "a pane with room to spare stays put"
    );
}

#[test]
fn test_a_refresh_reads_everything_the_view_shows_in_one_chain() {
    // Each read asks for the next, so one refresh fills the whole view
    // rather than five jobs racing each other into it.
    let mut editor = editor_with_diff();
    editor.on_job(reply(git_exec::TAG_STATUS, STATUS_OUTPUT));
    let after_log = editor.on_job(reply(git_exec::TAG_LOG, ""));
    assert_eq!(command_of(&after_log).tag, git_exec::TAG_STASHES);
    let after_stashes = editor.on_job(reply(git_exec::TAG_STASHES, ""));
    assert_eq!(command_of(&after_stashes).tag, git_exec::TAG_REMOTES);
    let after_remotes = editor.on_job(reply(git_exec::TAG_REMOTES, "origin\nmy/fork\n"));
    assert_eq!(editor.repo.remotes, ["origin", "my/fork"]);
    assert_eq!(command_of(&after_remotes).tag, git_exec::TAG_UNPULLED);

    // The chain ends on the state files, which no command reports.
    let after_unpulled = editor.on_job(reply(git_exec::TAG_UNPULLED, ""));
    let EditorOutcome::Job(JobRequest::ReadFiles(paths)) = after_unpulled else {
        panic!("expected the state files, got {after_unpulled:?}");
    };
    assert!(paths.iter().any(|path| path.ends_with("MERGE_HEAD")));

    // And the open diff is what still needs reading.
    let stale = editor.request_stale_diff();
    assert_eq!(command_of(&stale).tag, git_exec::TAG_DIFF);
}

#[test]
fn test_a_view_asked_for_afresh_starts_at_its_top() {
    let mut editor = loaded_editor();
    open_log(&mut editor);
    let log = log_output(&[("abc1234", "one"), ("def5678", "two"), ("0123abc", "three")]);
    editor.on_job(reply(git_exec::TAG_LOG_VIEW, &log));
    editor.on_key(&press_char('j'));
    editor.on_key(&press_char('j'));
    editor.on_job(reply(git_exec::TAG_CHANGED, ""));
    editor.on_key(&press_char('q'));
    open_log(&mut editor);
    editor.on_job(reply(git_exec::TAG_LOG_VIEW, &log));
    assert_eq!(editor.output.as_ref().expect("the log").list.cursor(), 0);
}

#[test]
fn test_read_only_output_with_nothing_in_it_says_so() {
    let mut editor = loaded_editor();
    editor.on_job(reply(git_exec::TAG_READ, "\n"));
    assert!(editor.output.is_none());
    assert_eq!(notice_text(&mut editor).as_deref(), Some("nothing to show"));
}

#[test]
fn test_a_revision_that_could_pass_for_an_option_is_refused() {
    let mut editor = loaded_editor();
    let outcome = editor.on_prompt(PromptReply {
        answer: Some("-f".to_string()),
        tag: ASK_BRANCH_CHECKOUT,
    });
    assert_eq!(outcome, EditorOutcome::Consumed, "nothing runs");
    assert!(notice_text(&mut editor).is_some_and(|text| text.contains("not a revision")));
}
