//! The git view's tests: pushing, pulling, fetching, and the remotes.

use super::*;

#[test]
fn test_a_remote_with_a_slash_in_its_name_is_split_from_its_branch_whole() {
    // Split at the first `/`, `my/fork/main` pulled `fork/main` from `my`,
    // and deleting it asked a remote that is not there.
    let mut editor = editor_with_diff();
    editor.repo.remotes = vec!["origin".to_string(), "my/fork".to_string()];
    assert_eq!(editor.repo.split_remote("my/fork/main"), Some(("my/fork", "main")));
    assert_eq!(editor.repo.split_remote("origin/a/b"), Some(("origin", "a/b")));
    assert_eq!(editor.repo.split_remote("nowhere/main"), None, "no such remote");
    editor.repo.status.upstream = Some("my/fork/feature".to_string());
    assert_eq!(editor.repo.upstream_remote(), Some("my/fork"));
    editor.repo.remotes.clear();
    assert_eq!(editor.repo.split_remote("origin/main"), Some(("origin", "main")), "before the list is read");
}

#[test]
fn test_fetching_and_pulling_ask_from_where() {
    let mut editor = loaded_editor();
    assert_eq!(
        command_of(&choose(&mut editor, 'f', 'u')).args,
        ["fetch", "--prune", "--end-of-options", "origin"],
        "the remote origin/main is on"
    );
    assert_eq!(
        command_of(&choose(&mut editor, 'f', 'a')).args,
        ["fetch", "--all", "--prune"]
    );
    assert_eq!(
        command_of(&choose(&mut editor, 'F', 'u')).args,
        ["pull", "--rebase"]
    );
    assert_eq!(
        command_of(&choose(&mut editor, 'F', 'm')).args,
        ["pull", "--no-rebase"]
    );
    choose(&mut editor, 'F', 'e');
    let pull = command_of(&editor.on_prompt(PromptReply {
        answer: Some("origin/feature".to_string()),
        tag: ASK_PULL_FROM,
    }));
    assert_eq!(pull.args, ["pull", "--rebase", "--end-of-options", "origin", "feature"]);
}

#[test]
fn test_fetching_from_the_upstream_of_a_branch_tracking_nothing_says_so() {
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(reply(git_exec::TAG_STATUS, "# branch.head main\n"));
    assert_eq!(choose(&mut editor, 'f', 'u'), EditorOutcome::Consumed);
    assert!(editor.take_notice().is_some());
}

#[test]
fn test_a_spin_off_checks_out_a_new_branch_and_puts_this_one_back_on_its_upstream() {
    let mut editor = loaded_editor();
    choose(&mut editor, 'b', 's');
    let checkout = answer(&mut editor, ASK_SPIN_OFF, "feature");
    assert_eq!(checkout.args, ["checkout", "-b", "feature"]);
    let reset = command_of(&answer_job(&mut editor, git_exec::TAG_SPIN_OFF, ""));
    assert_eq!(reset.args, ["branch", "--force", "main", "origin/main"]);

    choose(&mut editor, 'b', 'm');
    assert_eq!(answer(&mut editor, ASK_BRANCH_RENAME, "renamed").args, ["branch", "--move", "renamed"]);
}

#[test]
fn test_ctrl_shift_p_pushes_to_the_tracked_remote_and_keeps_tracking() {
    let mut editor = loaded_editor();
    let command = command_of(&editor.on_key(&quick_push_key()));
    assert_eq!(
        command.args,
        [
            "push",
            "--set-upstream",
            "--end-of-options",
            "origin",
            "main"
        ]
    );
}

#[test]
fn test_ctrl_shift_p_uses_the_remote_the_branch_tracks_not_always_origin() {
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(reply(
        git_exec::TAG_STATUS,
        "# branch.head feature/x\n# branch.upstream fork/feature/x\n",
    ));
    let command = command_of(&editor.on_key(&quick_push_key()));
    assert_eq!(
        command.args,
        [
            "push",
            "--set-upstream",
            "--end-of-options",
            "fork",
            "feature/x"
        ]
    );
}

#[test]
fn test_ctrl_shift_p_pushes_to_a_tracked_branch_of_another_name() {
    // Pushed under its own name, `feat` tracking `origin/feature` made a new
    // `origin/feat` and moved its upstream there.
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(reply(git_exec::TAG_STATUS, "# branch.head feat\n# branch.upstream origin/feature\n"));
    let command = command_of(&editor.on_key(&quick_push_key()));
    assert_eq!(command.args, ["push", "--set-upstream", "--end-of-options", "origin", "feat:feature"]);
}

#[test]
fn test_ctrl_shift_p_creates_the_branch_on_origin_when_nothing_is_tracked() {
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(reply(git_exec::TAG_STATUS, "# branch.head topic\n"));
    let command = command_of(&editor.on_key(&quick_push_key()));
    assert_eq!(
        command.args,
        [
            "push",
            "--set-upstream",
            "--end-of-options",
            "origin",
            "topic"
        ]
    );
}

#[test]
fn test_ctrl_shift_p_on_a_detached_head_says_so_and_runs_nothing() {
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(reply(git_exec::TAG_STATUS, "# branch.head (detached)\n"));
    assert_eq!(editor.on_key(&quick_push_key()), EditorOutcome::Consumed);
    assert_eq!(
        notice_text(&mut editor).as_deref(),
        Some("no branch to push")
    );
}

#[test]
fn test_ctrl_shift_alt_f_pulls_the_tracked_branch_with_rebase() {
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(reply(
        git_exec::TAG_STATUS,
        "# branch.head topic\n# branch.upstream fork/feature/x\n",
    ));
    let command = command_of(&editor.on_key(&quick_pull_key()));
    assert_eq!(
        command.args,
        ["pull", "--rebase", "--end-of-options", "fork", "feature/x"]
    );
}

#[test]
fn test_ctrl_shift_alt_f_pulls_the_same_name_from_origin_when_nothing_is_tracked() {
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(reply(git_exec::TAG_STATUS, "# branch.head topic\n"));
    let command = command_of(&editor.on_key(&quick_pull_key()));
    assert_eq!(
        command.args,
        ["pull", "--rebase", "--end-of-options", "origin", "topic"]
    );
}

#[test]
fn test_ctrl_shift_alt_f_on_a_detached_head_says_so_and_runs_nothing() {
    let mut editor = GitEditor::new(PathBuf::from("/repo"));
    editor.on_job(reply(git_exec::TAG_ROOT, "/repo\n/repo/.git\n"));
    editor.on_job(reply(git_exec::TAG_STATUS, "# branch.head (detached)\n"));
    assert_eq!(editor.on_key(&quick_pull_key()), EditorOutcome::Consumed);
    assert_eq!(
        notice_text(&mut editor).as_deref(),
        Some("no branch to pull")
    );
}

#[test]
fn test_a_remote_add_answer_splits_into_name_and_url() {
    let mut editor = loaded_editor();
    let EditorOutcome::Prompt(request) = choose(&mut editor, 'M', 'a') else {
        panic!("expected a prompt");
    };
    let command = command_of(&editor.on_prompt(PromptReply {
        answer: Some("upstream git@example.com:o/r.git".to_string()),
        tag: request.tag,
    }));
    assert_eq!(
        command.args,
        [
            "remote",
            "add",
            "--end-of-options",
            "upstream",
            "git@example.com:o/r.git"
        ]
    );
}

#[test]
fn test_a_malformed_remote_add_reports_instead_of_running() {
    let mut editor = loaded_editor();
    let EditorOutcome::Prompt(request) = choose(&mut editor, 'M', 'a') else {
        panic!("expected a prompt");
    };
    let outcome = editor.on_prompt(PromptReply {
        answer: Some("justaname".to_string()),
        tag: request.tag,
    });
    assert_eq!(outcome, EditorOutcome::Consumed);
    assert!(notice_text(&mut editor)
        .unwrap_or_default()
        .starts_with("failed:"));
}

#[test]
fn test_an_ssh_remote_becomes_an_https_url() {
    assert_eq!(
        browser_url("git@github.com:owner/repo.git").as_deref(),
        Some("https://github.com/owner/repo")
    );
    assert_eq!(
        browser_url("https://gitlab.com/owner/repo.git").as_deref(),
        Some("https://gitlab.com/owner/repo")
    );
    assert_eq!(browser_url("/srv/git/bare.git"), None);
}

#[test]
fn test_p_opens_the_push_menu_naming_where_each_push_goes() {
    let editor = push_menu(PUSH_CONFIG);
    let hint = editor.hint().expect("the menu");
    let said = |key: &str| {
        hint.items
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, what)| what.as_str())
    };
    assert_eq!(said("p"), Some("pushRemote, after setting that"));
    assert_eq!(said("u"), Some("origin/main"), "the upstream it tracks");
    assert_eq!(said("e"), Some("elsewhere"));
}

#[test]
fn test_switches_turned_on_in_the_options_go_with_the_push() {
    let mut editor = push_menu("remote.origin.url u\nbranch.main.pushremote origin\n");
    for key in ['-', 'f', 'd'] {
        editor.on_key(&press_char(key));
    }
    let options = editor.hint().expect("the options menu");
    assert!(options.items[0].1.starts_with("[x]"), "{:?}", options.items);
    editor.on_key(&press(KeyCode::Enter));
    assert!(editor
        .hint()
        .expect("back")
        .items
        .iter()
        .any(|(_, what)| what == "Options [--force-with-lease --dry-run]"));
    let push = command_of(&editor.on_key(&press_char('p')));
    assert_eq!(push.tag, git_exec::TAG_PUSH);
    assert_eq!(
        push.args,
        [
            "push",
            "--force-with-lease",
            "--dry-run",
            "--end-of-options",
            "origin",
            "main"
        ]
    );
}

#[test]
fn test_p_with_no_push_remote_sets_one_then_pushes_to_it() {
    let mut editor = push_menu(PUSH_CONFIG);
    let listing = command_of(&editor.on_key(&press_char('p')));
    assert_eq!(listing.tag, git_exec::TAG_CANDIDATES);
    let EditorOutcome::Pick(request) = editor.on_job(reply(git_exec::TAG_CANDIDATES, "origin\nfork\n"))
    else {
        panic!("a list of remotes");
    };
    let set = command_of(&editor.on_prompt(PromptReply {
        answer: Some("fork".to_string()),
        tag: request.tag,
    }));
    assert_eq!(set.tag, git_exec::TAG_PUSH_REMOTE_SET);
    assert!(set
        .args
        .ends_with(&["branch.main.pushRemote".to_string(), "fork".to_string()]));
    let push = command_of(&editor.on_job(reply(git_exec::TAG_PUSH_REMOTE_SET, "")));
    assert_eq!(push.args, ["push", "--end-of-options", "fork", "main"]);
}

#[test]
fn test_elsewhere_offers_a_new_branch_on_each_remote_first() {
    let mut editor = push_menu(PUSH_CONFIG);
    editor.on_key(&press_char('e'));
    let EditorOutcome::Pick(request) =
        editor.on_job(reply(git_exec::TAG_CANDIDATES, "origin/main\norigin/dev\n"))
    else {
        panic!("a list of destinations");
    };
    assert_eq!(request.items, ["fork/main", "origin/main", "origin/dev"]);
    let push = command_of(&editor.on_prompt(PromptReply {
        answer: Some("origin/dev".to_string()),
        tag: request.tag,
    }));
    assert_eq!(
        push.args,
        ["push", "--end-of-options", "origin", "main:dev"]
    );
}

#[test]
fn test_all_tags_go_to_the_upstream_s_remote() {
    let mut editor = push_menu(PUSH_CONFIG);
    let push = command_of(&editor.on_key(&press_char('t')));
    assert_eq!(push.args, ["push", "--tags", "--end-of-options", "origin"]);
}
