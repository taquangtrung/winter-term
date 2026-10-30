//! The git view's tests: stashing, and acting on a stash.

use super::*;

#[test]
fn test_stash_choices_run_the_matching_command() {
    let mut editor = loaded_editor();
    assert_eq!(
        command_of(&choose(&mut editor, 'z', 'p')).args,
        ["stash", "pop"]
    );
    assert_eq!(
        choose(&mut editor, 'z', 'd'),
        EditorOutcome::Consumed,
        "no stash, nothing to drop"
    );
    assert_eq!(
        command_of(&choose(&mut editor, 'z', 'l')).args,
        ["stash", "list"]
    );
}

#[test]
fn test_dropping_a_stash_from_the_menu_asks_first() {
    // `z d` dropped the newest stash on the spot, while `x` on the same
    // stash asked: a stash dropped cannot be taken back.
    let mut editor = stash_editor_on(Item::StashHeading);
    let EditorOutcome::Prompt(request) = choose(&mut editor, 'z', 'd') else {
        panic!("the drop asks first");
    };
    assert!(request.label.contains("stash@{0}"), "{}", request.label);
    let command = command_of(&editor.on_prompt(PromptReply {
        answer: Some("y".to_string()),
        tag: request.tag,
    }));
    assert_eq!(command.args, ["stash", "drop", "stash@{0}"]);
}

#[test]
fn test_the_stash_menu_stashes_parts_and_acts_on_the_stash_at_the_cursor() {
    let mut editor = loaded_editor();
    assert_eq!(
        command_of(&choose(&mut editor, 'z', 'i')).args,
        ["stash", "push", "--staged"]
    );
    assert_eq!(
        command_of(&choose(&mut editor, 'z', 'x')).args,
        ["stash", "push", "--keep-index"]
    );
    assert_eq!(
        command_of(&choose(&mut editor, 'z', 'p')).args,
        ["stash", "pop"],
        "off a stash, the newest"
    );
    let mut editor = stash_editor_on(Item::Stash(0));
    assert_eq!(
        command_of(&choose(&mut editor, 'z', 'p')).args,
        ["stash", "pop", "--index", "stash@{0}"]
    );
}

#[test]
fn test_a_stash_message_may_be_empty_because_it_is_optional() {
    let mut editor = loaded_editor();
    let EditorOutcome::Prompt(request) = choose(&mut editor, 'z', 'z') else {
        panic!("expected a prompt");
    };
    let command = command_of(&editor.on_prompt(PromptReply {
        answer: Some(String::new()),
        tag: request.tag,
    }));
    assert_eq!(command.args, ["stash", "push"], "no empty --message");
}

#[test]
fn test_enter_on_a_stash_reads_it_as_a_diff() {
    let mut editor = loaded_editor();
    editor.on_job(reply(
        git_exec::TAG_STASHES,
        "stash@{0}\u{1f}WIP on main: a thing\n",
    ));
    let at = editor
        .rows
        .iter()
        .position(|row| row.item == Item::Stash(0))
        .expect("the stash row");
    editor.list.set_cursor(at);

    let command = command_of(&editor.on_key(&press(KeyCode::Enter)));
    assert_eq!(command.args[0], "stash");
    assert!(
        command.args.iter().any(|arg| arg == "stash@{0}"),
        "the stash under the cursor, got {:?}",
        command.args
    );
}

#[test]
fn test_a_and_minus_over_a_stash_or_a_commit_act_on_it_uncommitted() {
    let mut editor = stash_editor_on(Item::Stash(0));
    let apply = command_of(&editor.on_key(&press_char('a')));
    assert_eq!(apply.args, ["stash", "apply", "--index", "stash@{0}"]);

    let mut editor = stash_editor_on(Item::Commit("abc1234".to_string()));
    let pick = command_of(&editor.on_key(&press_char('a')));
    assert_eq!(pick.args, ["cherry-pick", "--no-commit", "abc1234"]);
    let revert = command_of(&editor.on_key(&press_char('-')));
    assert_eq!(revert.args, ["revert", "--no-commit", "abc1234"]);
}

#[test]
fn test_x_over_a_stash_drops_it_and_over_the_heading_drops_them_all_once_confirmed() {
    for (item, args) in [
        (Item::Stash(0), vec!["stash", "drop", "stash@{0}"]),
        (Item::StashHeading, vec!["stash", "clear"]),
    ] {
        let mut editor = stash_editor_on(item);
        let asked = editor.on_key(&press_char('x'));
        assert!(matches!(asked, EditorOutcome::Prompt(_)), "asks first: {asked:?}");
        let command = command_of(&editor.on_prompt(PromptReply {
            answer: Some("y".to_string()),
            tag: ASK_DISCARD,
        }));
        assert_eq!(command.args, args);
    }
}
