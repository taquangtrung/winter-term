//! The window's editor tests: prompts, lists, and the close and quit questions.

use super::*;

#[test]
fn test_closing_a_pane_buries_its_editors_and_asks_nothing_about_them() {
    let mut app = App::new();
    let tab = app.show_editor(EditorKind::Text, Box::new(UnsavedEditor("main.rs")));
    let sibling = app.alloc_tab_id();
    app.layout_mut()
        .split(Direction::Vertical, SPLIT_RATIO, sibling);

    app.close_pane(tab);
    assert!(
        app.chrome.close_prompt().is_none(),
        "unsaved text is not lost"
    );
    assert_eq!(app.layout().shown_tabs(), vec![sibling], "the pane closed");
    assert!(app.open_tabs.buried().contains(&tab));
    assert!(app
        .open_tabs
        .page(&tab)
        .unwrap()
        .page
        .as_editor()
        .is_some_and(|editor| editor.is_dirty()));
}

#[test]
fn test_kill_buffer_asks_about_unsaved_text_then_lets_it_go() {
    let mut app = App::new();
    let tab = app.show_editor(EditorKind::Text, Box::new(UnsavedEditor("main.rs")));

    app.kill_buffer(tab);
    let asked = app.chrome.close_prompt().expect("the question is up");
    assert!(
        asked.label.contains("main.rs"),
        "naming it: {}",
        asked.label
    );
    let view = app.input_view().expect("input view is up");
    assert_eq!(view.kind, Some(winter_render::NoticeKind::Warning));
    assert_eq!(view.title.as_deref(), Some("Discard Unsaved Changes?"));
    assert_eq!(
        asked.label,
        "Not saved: main.rs. Closing the buffer loses the edits."
    );
    app.handle_close_prompt_key(&press(KeyCode::Char('n')));
    assert!(app.layout().contains(tab), "no leaves it be");

    app.kill_buffer(tab);
    app.handle_close_prompt_key(&press(KeyCode::Char('y')));
    assert!(!app.layout().contains(tab));
    assert!(!app.open_tabs.is_page(&tab), "and yes lets it go");
    assert!(app.open_tabs.buried().is_empty(), "rather than burying it");
}

#[test]
fn test_quitting_asks_about_unsaved_text_in_a_buried_buffer() {
    let mut app = App::new();
    let tab = app.show_editor(EditorKind::Text, Box::new(UnsavedEditor("main.rs")));
    app.close_editor(tab);
    assert!(
        app.ask_before_quitting(),
        "the buried edits are asked about"
    );
    let asked = app.chrome.close_prompt().expect("the question is up");
    assert!(
        asked.label.contains("main.rs"),
        "naming it: {}",
        asked.label
    );
}

#[test]
fn test_closing_a_tab_with_a_program_running_asks_first_and_an_idle_one_does_not() {
    let mut idle = App::new();
    let tab = terminal_tab(&mut idle, false);
    assert!(
        !idle.ask_before_closing_tab(tab),
        "a shell at its prompt just closes"
    );
    assert!(idle.chrome.editor_prompt().is_none());

    let mut busy = App::new();
    let tab = terminal_tab(&mut busy, true);
    assert!(busy.ask_before_closing_tab(tab));
    let label = busy.chrome.close_prompt().expect("asked").label.clone();
    assert!(label.starts_with("Still running: "), "label: {label}");
    assert!(
        label.ends_with("Closing the tab stops it."),
        "label: {label}"
    );
    let title = busy.input_view().and_then(|view| view.title);
    assert_eq!(
        title.as_deref(),
        Some("Stop Running Program?"),
        "a program, not unsaved edits"
    );
    let yes = busy
        .input_view()
        .and_then(|view| view.buttons.first().map(|button| button.text.clone()));
    assert_eq!(
        yes.as_deref(),
        Some("Stop"),
        "the yes button says what it does"
    );
    let focused = busy
        .chrome
        .close_prompt()
        .map(|prompt| prompt.buttons.focused());
    assert_eq!(
        focused,
        Some(1),
        "Enter alone cancels a close that ends a program"
    );
    assert_eq!(
        busy.chrome.close_prompt().map(|prompt| prompt.pending),
        Some(PendingClose::Tab(tab))
    );
}

#[test]
fn test_quitting_with_a_program_running_asks_and_a_yes_goes_through_without_asking_again() {
    let mut app = App::new();
    terminal_tab(&mut app, true);

    assert!(app.ask_before_quitting());
    assert_eq!(
        app.chrome.close_prompt().map(|prompt| prompt.pending),
        Some(PendingClose::Quit)
    );
    assert!(!app.runtime.exit.requested, "nothing has been closed yet");

    app.handle_close_prompt_key(&Key::plain(KeyCode::Char('y')));

    assert!(app.runtime.exit.requested);
    assert!(
        app.runtime.exit.quit_confirmed,
        "the quit that follows is not asked about again"
    );
}

#[test]
fn test_quitting_declined_keeps_the_window() {
    let mut app = App::new();
    terminal_tab(&mut app, true);
    app.ask_before_quitting();

    app.handle_close_prompt_key(&Key::plain(KeyCode::Char('n')));

    assert!(!app.runtime.exit.requested);
    assert!(!app.runtime.exit.quit_confirmed);
    assert!(app.chrome.close_prompt().is_none());
}

#[test]
fn test_quitting_an_idle_window_asks_nothing() {
    let mut app = App::new();
    terminal_tab(&mut app, false);
    assert!(!app.ask_before_quitting());
}

#[test]
fn test_the_last_shell_exiting_with_unsaved_edits_keeps_the_window() {
    // The dead last tab asked to quit, the quit asked about the buried
    // edits, and declining left the dead tab to ask again every frame.
    let mut app = crate::window::test_support::app_with_tabs(1);
    let shell = app.layout().shown_tabs()[0];
    let unsaved = app.show_editor(EditorKind::Text, Box::new(UnsavedEditor("notes")));
    app.close_tab_now(unsaved, Closing::Keep);
    assert_eq!(
        app.buried_tabs(),
        vec![unsaved],
        "the edits are buried, not lost"
    );
    crate::window::test_support::insert_dead_pane(&mut app, shell);
    app.reap_dead_panes();
    assert!(!app.runtime.exit.requested, "no quit to ask about");
    let members = app.layout().members();
    assert_eq!(members.len(), 1);
    assert_ne!(members[0], shell, "a fresh shell took the dead one's place");
    assert!(app.open_tabs.terminal(&members[0]).is_some());
}

#[test]
fn test_saying_yes_to_closing_the_last_tab_still_asks_about_buried_edits() {
    // The yes about the tab was taken as a yes to the quit, so buried
    // edits went with the window unasked.
    let mut app = crate::window::test_support::app_with_tabs(1);
    let unsaved = app.show_editor(EditorKind::Text, Box::new(UnsavedEditor("notes")));
    app.close_tab_now(unsaved, Closing::Keep);
    let last = app.layout().shown_tabs()[0];
    app.answer_close_prompt(PendingClose::Tab(last), true);
    assert!(app.runtime.exit.requested, "the last tab closing quits");
    assert!(
        !app.runtime.exit.quit_confirmed,
        "but the quit is still asked about"
    );
}

#[test]
fn test_a_close_question_goes_with_the_tab_it_was_about() {
    // Regression: the question went when its tab did, but what it would
    // close stayed pending, and the next editor question answered was
    // taken as the answer to it.
    let (mut app, asking, heard) = app_asking();
    app.chrome.overlay = None;
    let unsaved = app.show_editor(EditorKind::Text, Box::new(WaitedEditor));
    assert!(app.ask_before_closing_tab(unsaved));
    app.close_tab_now(unsaved, Closing::Keep);
    assert!(
        app.chrome.close_prompt().is_none(),
        "nothing left to ask about"
    );

    app.offer_key_to_editor(asking, &press(KeyCode::Char('x')));
    answer(&mut app, "y");
    assert_eq!(heard.borrow().as_slice(), [Some("seedy".to_string())]);
    assert!(!app.runtime.exit.requested);
}

#[test]
fn test_an_editor_question_cannot_hide_the_hosts_close_question() {
    let (mut app, asking, heard) = app_asking();
    app.chrome.overlay = None;
    let unsaved = app.show_editor(EditorKind::Text, Box::new(WaitedEditor));
    assert!(app.ask_before_closing_tab(unsaved));

    app.offer_key_to_editor(asking, &press(KeyCode::Char('x')));
    assert!(
        app.chrome.close_prompt().is_some(),
        "the host's question stays up"
    );
    assert_eq!(
        heard.borrow().as_slice(),
        [None],
        "and the editor hears that its own went unanswered"
    );
}

#[test]
fn test_the_hosts_question_waits_for_the_editor_question_it_finds_open() {
    // The editor's question stays up until it is answered, and is never
    // dropped for the close question, which opens after it.
    let (mut app, _, heard) = app_asking();
    let unsaved = app.show_editor(EditorKind::Text, Box::new(WaitedEditor));
    assert!(app.ask_before_closing_tab(unsaved));
    assert!(
        app.chrome.editor_prompt().is_some(),
        "the editor's question stays"
    );
    assert!(heard.borrow().is_empty(), "and is not answered for it");
    app.handle_prompt_key(&press(KeyCode::Escape));
    app.open_held_dialog();
    assert!(
        app.chrome.close_prompt().is_some(),
        "the close question follows"
    );
}

#[test]
fn test_a_reload_over_unsaved_edits_asks_first_and_a_yes_carries_it_out() {
    // Regression: a reload, from the palette or another process, dropped
    // edits on no disk without asking.
    let mut app = App::new();
    assert!(
        !app.ask_before_reloading(),
        "nothing to lose, nothing asked"
    );
    app.show_editor(EditorKind::Text, Box::new(UnsavedEditor("main.rs")));
    assert!(app.ask_before_reloading());
    let label = app.chrome.close_prompt().expect("asked").label.clone();
    assert!(label.contains("main.rs"), "label: {label}");

    app.handle_close_prompt_key(&press(KeyCode::Char('y')));
    assert!(app.runtime.exit.reload_pending, "the reload goes ahead");
    assert!(app.runtime.exit.reload_confirmed, "without asking again");
}

#[test]
fn test_a_prompt_starts_holding_its_initial_text() {
    let (app, _, _) = app_asking();
    let view = app.input_view().expect("a dialog");
    assert_eq!(view.input.as_deref(), Some("seed"));
    assert_eq!(view.label, "Name", "the label's separator is the layout's");
}

#[test]
fn test_a_question_answered_by_one_key_offers_no_line_to_type_on() {
    // A caret on a line that cannot be typed into invites typing into it.
    let text = input_dialog("Name: ", "seed", 4, PromptMode::Text, Vec::new());
    assert_eq!(text.input.as_deref(), Some("seed"));

    let confirm = input_dialog(
        "Delete 3 entries? (y/n) ",
        "",
        0,
        PromptMode::Confirm,
        ButtonGroup::confirm().to_view(),
    );
    assert_eq!(confirm.input, None);
    assert_eq!(confirm.label, "Delete 3 entries?", "the buttons say y/n");
    assert_eq!(confirm.buttons.len(), 2, "confirmation has buttons");
    assert!(confirm.buttons[0].focused, "first button is focused");
    assert_eq!(
        confirm.keys.first().map(|(key, _)| key.as_str()),
        Some("y"),
        "a dialog taking one key says which"
    );
}

#[test]
fn test_typing_edits_the_prompt_and_enter_delivers_the_answer() {
    let (mut app, pane, heard) = app_asking();
    app.handle_prompt_key(&press(KeyCode::Backspace));
    app.handle_prompt_key(&press(KeyCode::Char('!')));
    assert_eq!(
        app.input_view().and_then(|view| view.input).as_deref(),
        Some("see!")
    );

    app.handle_prompt_key(&press(KeyCode::Enter));
    assert!(
        app.chrome.editor_prompt().is_none(),
        "the prompt closes on Enter"
    );
    assert!(app.open_tabs.is_page(&pane), "and the page stays open");
    assert_eq!(heard.borrow().as_slice(), [Some("see!".to_string())]);
}

#[test]
fn test_a_prompt_answer_takes_the_emacs_editing_keys() {
    let (mut app, _pane, heard) = app_asking();
    let ctrl = |c| Key {
        ctrl: true,
        ..press(KeyCode::Char(c))
    };
    app.handle_prompt_key(&ctrl('a'));
    app.handle_prompt_key(&press(KeyCode::Char('>')));
    assert_eq!(
        app.input_view().map(|view| view.caret),
        Some(1),
        "the caret is mid-line"
    );
    app.handle_prompt_key(&ctrl('k'));
    app.handle_prompt_key(&ctrl('y'));
    app.handle_prompt_key(&press(KeyCode::Enter));
    assert_eq!(heard.borrow().as_slice(), [Some(">seed".to_string())]);
}

#[test]
fn test_confirmation_prompt_tab_switches_buttons_and_enter_submits_focused() {
    let (mut app, pane, heard) = app_asking();
    app.act_on_editor_outcome(
        pane,
        EditorOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: "Delete file?".to_string(),
            mode: PromptMode::Confirm,
            tag: "delete",
            title: None,
        }),
    );
    heard.borrow_mut().clear();
    let view = app.input_view().expect("confirmation dialog");
    assert_eq!(view.buttons.len(), 2);
    assert!(view.buttons[0].focused, "Yes starts focused");

    // Tab moves focus to No
    app.handle_prompt_key(&press(KeyCode::Tab));
    let view = app.input_view().expect("still open");
    assert!(!view.buttons[0].focused);
    assert!(view.buttons[1].focused, "No is now focused");

    // Enter submits the focused button (No -> None)
    app.handle_prompt_key(&press(KeyCode::Enter));
    assert!(app.input_view().is_none(), "dialog closed");
    assert_eq!(
        heard.borrow().as_slice(),
        [None],
        "declined via focused No button"
    );
}

#[test]
fn test_close_prompt_tab_switches_buttons_and_enter_submits_focused() {
    let mut app = App::new();
    let tab = app.show_editor(EditorKind::Text, Box::new(WaitedEditor));
    app.close_pane(tab);
    assert!(app.chrome.close_prompt().is_some());
    let view = app.input_view().expect("dialog open");
    assert!(
        view.buttons[1].focused,
        "Cancel starts focused: the close loses something"
    );

    // Tab moves to the yes button, and back
    app.handle_close_prompt_key(&press(KeyCode::Tab));
    let view = app.input_view().expect("dialog stays open");
    assert!(view.buttons[0].focused, "the yes button is now focused");
    app.handle_close_prompt_key(&press(KeyCode::Tab));

    // Enter on Cancel cancels the close
    app.handle_close_prompt_key(&press(KeyCode::Enter));
    assert!(app.chrome.close_prompt().is_none(), "dialog closed");
    assert_eq!(app.layout().shown_tabs().len(), 1, "pane was not closed");
}

#[test]
fn test_close_prompt_mouse_click_and_hover() {
    let mut app = App::new();
    let tab = app.show_editor(EditorKind::Text, Box::new(WaitedEditor));
    let sibling = app.alloc_tab_id();
    app.layout_mut()
        .split(Direction::Vertical, SPLIT_RATIO, sibling);

    app.close_pane(tab);
    assert!(app.chrome.close_prompt().is_some());
    let view = app.input_view().expect("dialog open");
    assert!(view.buttons[1].focused, "Cancel starts focused");

    // Hover button 0 (the yes button), then button 1 (Cancel)
    assert!(app.chrome.hover_dialog_button(0));
    assert!(app.chrome.hover_dialog_button(1));
    let view = app.input_view().expect("dialog open");
    assert!(view.buttons[1].focused, "Cancel is focused again");

    // Hover button 1 again returns false (already focused)
    assert!(!app.chrome.hover_dialog_button(1));

    // Click button 1 (No) cancels the close
    app.click_dialog_button(1);
    assert!(
        app.chrome.close_prompt().is_none(),
        "dialog closed on click"
    );
    assert_eq!(app.layout().shown_tabs().len(), 2, "pane was kept");

    // Try again with click button 0 (Yes)
    app.close_pane(tab);
    assert!(app.chrome.close_prompt().is_some());
    app.click_dialog_button(0);
    assert!(
        app.chrome.close_prompt().is_none(),
        "dialog closed on Yes click"
    );
    assert_eq!(app.layout().shown_tabs(), vec![sibling], "pane was closed");
}

#[test]
fn test_up_brings_back_the_last_answer_to_the_same_question() {
    let (mut app, pane, heard) = app_asking();
    app.handle_prompt_key(&press(KeyCode::Char('!')));
    app.handle_prompt_key(&press(KeyCode::Enter));
    let asked = || winter_model::editor::PromptRequest {
        initial: String::new(),
        label: "Name: ".to_string(),
        mode: PromptMode::Text,
        tag: "ask",
        title: None,
    };
    app.act_on_editor_outcome(pane, EditorOutcome::Prompt(asked()));
    app.handle_prompt_key(&press(KeyCode::Up));
    assert_eq!(
        app.input_view().and_then(|view| view.input).as_deref(),
        Some("seed!")
    );
    app.handle_prompt_key(&press(KeyCode::Down));
    assert_eq!(
        app.input_view().and_then(|view| view.input).as_deref(),
        Some("")
    );
    app.handle_prompt_key(&press(KeyCode::Up));
    app.handle_prompt_key(&press(KeyCode::Enter));
    assert_eq!(heard.borrow().len(), 2);
}

#[test]
fn test_a_secret_prompt_shows_dots_and_is_never_recalled() {
    let (mut app, pane, heard) = app_asking();
    app.handle_prompt_key(&press(KeyCode::Escape));
    let secret = || winter_model::editor::PromptRequest {
        initial: String::new(),
        label: "Password: ".to_string(),
        mode: PromptMode::Secret,
        tag: "ask",
        title: None,
    };
    app.act_on_editor_outcome(pane, EditorOutcome::Prompt(secret()));
    for c in "pw".chars() {
        app.handle_prompt_key(&press(KeyCode::Char(c)));
    }
    assert_eq!(
        app.input_view().and_then(|view| view.input).as_deref(),
        Some("••"),
        "how much is typed shows, never what"
    );
    app.handle_prompt_key(&press(KeyCode::Enter));
    assert_eq!(heard.borrow().last(), Some(&Some("pw".to_string())));

    // The same question asked in the open finds nothing to bring back.
    let text = winter_model::editor::PromptRequest {
        mode: PromptMode::Text,
        ..secret()
    };
    app.act_on_editor_outcome(pane, EditorOutcome::Prompt(text));
    app.handle_prompt_key(&press(KeyCode::Up));
    assert_eq!(
        app.input_view().and_then(|view| view.input).as_deref(),
        Some(""),
        "a password is not kept to be brought back"
    );
}

#[test]
fn test_the_space_bar_types_a_space_into_a_prompt() {
    // Regression: it has a key code of its own, which the prompt did not
    // match, so a search or a name could not hold a space.
    let (mut app, _pane, _heard) = app_asking();
    app.handle_prompt_key(&press(KeyCode::Space));
    app.handle_prompt_key(&press(KeyCode::Char('x')));
    assert_eq!(
        app.input_view().and_then(|view| view.input).as_deref(),
        Some("seed x")
    );
}

#[test]
fn test_a_list_answers_the_editor_the_way_a_prompt_does() {
    // The palette stands in for the prompt when the answer is one of a
    // known set: what is chosen comes back under the same tag.
    let (mut app, pane, heard) = app_asking();
    app.chrome.overlay = None;
    app.act_on_editor_outcome(
        pane,
        EditorOutcome::Pick(winter_model::editor::PickRequest {
            icons: Vec::new(),
            items: vec!["main".to_string(), "feature".to_string()],
            label: "Checkout".to_string(),
            placement: winter_model::editor::PickPlacement::Centered,
            tag: "ask",
        }),
    );
    assert!(app.chrome.picker().is_some(), "the list is showing");
    assert_eq!(
        app.chrome.picker().map(|picker| picker.view().title),
        Some("Checkout".to_string()),
        "and it says what it is a list of"
    );

    app.answer_editor_pick(Some("feature".to_string()));
    assert!(
        app.chrome.editor_pick().is_none(),
        "the question is answered"
    );
    assert_eq!(heard.borrow().as_slice(), [Some("feature".to_string())]);
}

#[test]
fn test_a_list_closed_without_choosing_says_so() {
    let (mut app, pane, heard) = app_asking();
    app.chrome.overlay = None;
    app.act_on_editor_outcome(
        pane,
        EditorOutcome::Pick(winter_model::editor::PickRequest {
            icons: Vec::new(),
            items: vec!["main".to_string()],
            label: "Checkout".to_string(),
            placement: winter_model::editor::PickPlacement::Centered,
            tag: "ask",
        }),
    );
    app.answer_editor_pick(None);
    assert_eq!(heard.borrow().as_slice(), [None]);
}

#[test]
fn test_escape_cancels_a_prompt_and_says_so() {
    // The editor has to hear the cancellation, not just stop hearing: an
    // operation waiting on an answer would otherwise stay half-started.
    let (mut app, _, heard) = app_asking();
    app.handle_prompt_key(&press(KeyCode::Escape));
    assert!(app.chrome.editor_prompt().is_none());
    assert_eq!(heard.borrow().as_slice(), [None]);
}

#[test]
fn test_closing_the_editor_takes_its_question_with_it() {
    // A prompt left behind would deliver its answer to an editor that is gone,
    // or worse, to whatever editor opened next in that pane.
    let (mut app, pane, heard) = app_asking();
    assert!(app.chrome.editor_prompt().is_some());
    app.close_editor(pane);
    assert!(app.chrome.editor_prompt().is_none());
    assert!(heard.borrow().is_empty(), "and answers nothing");
}

#[test]
fn test_a_window_command_a_file_s_editor_asks_for_leaves_it_open() {
    // A file's editor opening its preview or building its document is not
    // done with the file, as the command list is once a command is picked.
    let mut app = App::new();
    let dir = std::env::temp_dir().join(format!("winter-run-here-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("a.md");
    std::fs::write(&file, "# hi\n").expect("file");
    let editor = winter_text::TextEditor::new(file, None).expect("opens");
    let tab = app.show_editor(EditorKind::Text, Box::new(editor));
    app.act_on_editor_outcome(
        tab,
        EditorOutcome::RunActionHere("markdown_preview".to_string()),
    );
    assert!(app.open_tabs.is_page(&tab), "the file's tab is still open");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_dialog_stays_up_when_another_editor_asks_something() {
    let mut app = App::new();
    let tab = app.show_editor(EditorKind::Text, Box::new(UnsavedEditor("main.rs")));
    let other = app.show_editor(EditorKind::Text, Box::new(UnsavedEditor("notes")));
    let ask = |label: &str| {
        EditorOutcome::Prompt(winter_model::editor::PromptRequest::confirm("ask", label))
    };
    app.act_on_editor_outcome(tab, ask("first?"));
    app.act_on_editor_outcome(other, ask("second?"));
    let label = app
        .chrome
        .editor_prompt()
        .map(|prompt| prompt.request.label.clone());
    assert!(
        label.is_some_and(|label| label.starts_with("First?")),
        "the first question is still up"
    );
}

#[test]
fn test_a_dialog_of_the_window_s_own_waits_for_the_open_one() {
    let mut app = App::new();
    let tab = app.show_editor(EditorKind::Text, Box::new(UnsavedEditor("main.rs")));
    app.kill_buffer(tab);
    assert!(app.chrome.close_prompt().is_some());
    app.open_overlay(crate::window::Overlay::ThemeName(
        winter_model::textedit::field::TextField::line(""),
    ));
    assert!(app.chrome.close_prompt().is_some(), "the open dialog stays");
    app.handle_close_prompt_key(&press(KeyCode::Char('c')));
    assert!(app.chrome.close_prompt().is_none(), "c cancels");
    app.open_held_dialog();
    assert!(
        app.chrome.theme_name_input().is_some(),
        "the held one opens after"
    );
}
