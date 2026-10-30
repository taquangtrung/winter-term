//! The git view's tests: the log, and what is run from it.

use super::*;

#[test]
fn test_a_status_read_is_followed_by_the_recent_log() {
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    let next = editor.on_job(reply(git_exec::TAG_STATUS, STATUS_OUTPUT));
    assert_eq!(command_of(&next).tag, git_exec::TAG_LOG);
}

#[test]
fn test_the_refs_view_reads_a_branch_s_log_and_deletes_each_kind_of_ref() {
    let mut status = loaded_editor();
    status.on_key(&press_char('y'));
    let mut editor = view_tab(status.on_key(&press_char('r')));
    let listed = " \u{1f}refs/heads/topic\u{1f}def5678\u{1f}two\n \u{1f}refs/remotes/origin/old\u{1f}abc1234\u{1f}one\n \u{1f}refs/tags/v1\u{1f}abc1234\u{1f}one\n";
    editor.on_job(reply(git_exec::TAG_REFS, listed));
    let on = |editor: &mut GitEditor, name: &str| {
        let output = editor.output.as_mut().expect("the refs");
        let at = output
            .rows
            .iter()
            .position(|row| matches!(&row.item, Item::Ref(entry) if entry.name == name))
            .expect("the ref's row");
        output.list.set_cursor(at);
    };
    on(&mut editor, "topic");
    let log = editor.on_key(&press(KeyCode::Enter));
    let log = command_of(&log);
    assert_eq!(log.tag, git_exec::TAG_LOG_VIEW);
    assert_eq!(log.args.last().map(String::as_str), Some("topic"));

    for (name, args) in [
        ("topic", vec!["branch", "-d", "topic"]),
        ("origin/old", vec!["push", "--delete", "--end-of-options", "origin", "old"]),
        ("v1", vec!["tag", "--delete", "v1"]),
    ] {
        on(&mut editor, name);
        let EditorOutcome::Prompt(asked) = editor.on_key(&press_char('x')) else {
            panic!("{name}: asked first");
        };
        // A remote branch's deletion reaches the server, and the question
        // says so rather than reading like the local list's.
        assert_eq!(asked.label.contains("server origin"), name == "origin/old", "{}", asked.label);
        let command = command_of(&editor.on_prompt(PromptReply {
            answer: Some("y".to_string()),
            tag: ASK_DISCARD,
        }));
        assert_eq!(command.args, args, "{name}");
    }
}

#[test]
fn test_a_log_fills_the_view_and_can_ask_for_more_of_the_same_log() {
    let mut status = loaded_editor();
    let mut editor = view_tab(choose(&mut status, 'l', 'a'));
    let page = editor.log_count;
    // One commit past the page came back: there are more to load.
    let outcome = editor.on_job(reply(git_exec::TAG_LOG_VIEW, &commits_output(page + 1)));
    assert_eq!(outcome, EditorOutcome::Consumed);
    assert!(editor.output.is_some(), "the log fills its tab");
    assert!(offers_more(&editor));

    let more = command_of(&editor.on_key(&press_char('+')));
    assert!(editor.log_count > page, "asked for more");
    assert_eq!(more.tag, git_exec::TAG_LOG_VIEW);
    assert!(more.args.iter().any(|arg| arg == "--all"), "still of all refs: {:?}", more.args);
    let count = format!("--max-count={}", git_exec::with_peek(editor.log_count));
    assert!(more.args.contains(&count), "{:?}", more.args);
}

#[test]
fn test_the_log_menu_s_scopes_switches_and_searches() {
    let mut editor = loaded_editor();
    editor.on_key(&menu_key('l'));
    editor.on_key(&press_char('-'));
    editor.on_key(&press_char('m'));
    let local = command_of(&editor.on_key(&press_char('L')));
    assert!(local.args.iter().any(|arg| arg == "--no-merges"), "{:?}", local.args);
    assert_eq!(local.args.last().map(String::as_str), Some("--branches"));

    choose(&mut editor, 'l', 'g');
    let search = command_of(&editor.on_prompt(PromptReply {
        answer: Some("-fn main".to_string()),
        tag: ASK_LOG_REGEX,
    }));
    assert!(search.args.iter().any(|arg| arg == "-G-fn main"), "joined to its flag: {:?}", search.args);
}

#[test]
fn test_a_log_that_reaches_the_first_commit_offers_no_more() {
    // A history exactly a page long has its first commit on screen; the
    // old check took any full page for "there are more".
    let mut editor = loaded_editor();
    choose(&mut editor, 'l', 'l');
    let page = editor.log_count;
    editor.on_job(reply(git_exec::TAG_LOG_VIEW, &commits_output(page)));
    assert!(!offers_more(&editor));
    assert!(!matches!(
        editor.on_key(&press_char('+')),
        EditorOutcome::Job(_)
    ));
    assert_eq!(editor.log_count, page, "nothing asked for");
}

#[test]
fn test_q_leaves_the_log_for_the_status_view_rather_than_closing() {
    // Closing the whole editor on `q` from a log would lose the view the user
    // came from.
    let mut editor = loaded_editor();
    choose(&mut editor, 'l', 'l');
    editor.on_job(reply(git_exec::TAG_LOG_VIEW, &log_output(&[("aaa", "one")])));
    let outcome = editor.on_key(&press_char('q'));
    assert_eq!(
        command_of(&outcome).tag,
        git_exec::TAG_STATUS,
        "back at the status, it is read again at once"
    );
    assert!(editor.output.is_none());
    assert_ne!(editor.on_key(&press(KeyCode::Escape)), EditorOutcome::Close);
}

#[test]
fn test_r_i_in_the_log_rebases_from_the_commit_at_point() {
    let mut editor = loaded_editor();
    choose(&mut editor, 'l', 'l');
    editor.on_job(reply(
        git_exec::TAG_LOG_VIEW,
        &log_output(&[("abc1234", "one"), ("def5678", "two")]),
    ));
    while editor.revision_at_cursor().is_none() {
        assert_eq!(editor.on_key(&press_char('j')), EditorOutcome::Consumed);
    }
    let commit = editor.revision_at_cursor().expect("on a commit");
    let request = command_of(&choose(&mut editor, 'r', 'i'));
    assert_eq!(request.tag, git_exec::TAG_REBASE_FROM);
    assert!(request.args.contains(&commit), "{:?}", request.args);
    assert!(editor.output.is_some(), "the log stays up");
}

#[test]
fn test_c_w_in_the_log_rewords_the_commit_at_point() {
    let mut editor = loaded_editor();
    choose(&mut editor, 'l', 'l');
    editor.on_job(reply(
        git_exec::TAG_LOG_VIEW,
        &log_output(&[("abc1234", "one"), ("def5678", "two")]),
    ));
    while editor.revision_at_cursor().is_none() {
        editor.on_key(&press_char('j'));
    }
    editor.on_key(&press_char('j'));
    editor.on_key(&press_char('j'));
    let commit = editor.revision_at_cursor().expect("on the second commit");
    let request = command_of(&choose(&mut editor, 'c', 'w'));
    assert_eq!(request.tag, git_exec::TAG_REWORD_FROM);
    assert!(request.args.contains(&commit), "{:?}", request.args);
}

#[test]
fn test_the_status_reads_a_commit_or_a_log_in_a_tab_of_its_own() {
    let mut status = loaded_editor();
    let log = open_log(&mut status);
    assert!(matches!(log, EditorOutcome::GitView(_)), "{log:?}");
    assert!(status.output.is_none(), "the status stays the status");
    let tab = view_tab(log.clone());
    let EditorOutcome::GitView(view) = log else { unreachable!() };
    assert!(tab.shows_view(&view.request), "asked again, it is gone to");
    assert!(!status.shows_view(&view.request));
}

#[test]
fn test_a_log_s_commit_opens_in_another_tab_and_more_of_it_stays_here() {
    let mut status = loaded_editor();
    let mut editor = view_tab(open_log(&mut status));
    let commits: Vec<(String, String)> = (0..LOG_PAGE + 1)
        .map(|at| (format!("{at:07x}"), format!("commit {at}")))
        .collect();
    let commits: Vec<(&str, &str)> = commits.iter().map(|(hash, subject)| (hash.as_str(), subject.as_str())).collect();
    editor.on_job(reply(git_exec::TAG_LOG_VIEW, &log_output(&commits)));
    let at = editor
        .output
        .as_ref()
        .expect("the log")
        .rows
        .iter()
        .position(|row| matches!(row.item, Item::Commit(_)))
        .expect("a commit row");
    editor.output.as_mut().expect("the log").list.set_cursor(at);
    let commit = editor.on_key(&press(KeyCode::Enter));
    assert!(
        matches!(&commit, EditorOutcome::GitView(view) if matches!(&view.request, JobRequest::Command(c) if c.tag == git_exec::TAG_SHOW)),
        "{commit:?}"
    );
    let more = command_of(&editor.on_key(&press_char('+')));
    assert_eq!(more.tag, git_exec::TAG_LOG_VIEW);
}

#[test]
fn test_a_command_run_from_the_log_reads_the_log_again_and_keeps_the_place() {
    // A reword or a rebase from the log rewrites the commits it lists, and a
    // log left as it was would offer the old ones to act on next.
    let mut status = loaded_editor();
    let opened = open_log(&mut status);
    let asked = command_of(&opened);
    let mut editor = view_tab(opened);
    let log = log_output(&[("abc1234", "one"), ("def5678", "two"), ("0123abc", "three")]);
    editor.on_job(reply(git_exec::TAG_LOG_VIEW, &log));
    editor.on_key(&press_char('j'));
    editor.on_key(&press_char('j'));
    let place = editor.output.as_ref().expect("the log").list.cursor();
    assert!(place > 0);

    let reread = command_of(&editor.on_job(reply(git_exec::TAG_CHANGED, "")));
    assert_eq!(reread, asked, "the log, asked as it was");
    editor.on_job(reply(git_exec::TAG_LOG_VIEW, &log));
    assert_eq!(
        editor.output.as_ref().expect("the log").list.cursor(),
        place
    );

    // Leaving the log closes its tab, back to the status it came from.
    assert_eq!(editor.on_key(&press_char('q')), EditorOutcome::Close);
}
