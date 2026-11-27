//! The window's editor tests: keys an editor takes or leaves to the window, and saving on leaving.

use std::ops::RangeInclusive;

use super::*;

#[test]
fn test_the_stop_key_stops_a_busy_editor_and_is_the_editor_s_own_when_idle() {
    let (mut app, tab) = app_with_open_editor();
    // Idle: nothing to stop, so the key is offered to the editor, which
    // declines it.
    assert!(!app.offer_key_to_editor(tab, &STOP_TASKS_KEY));
    app.host.jobs.spawn(
        tab,
        JobRequest::Command(CommandRequest {
            id: 0,
            args: vec!["30".to_string()],
            cwd: PathBuf::from("."),
            env: Vec::new(),
            program: "sleep".to_string(),
            stdin: None,
            tag: "test",
        }),
    );
    assert!(app.offer_key_to_editor(tab, &STOP_TASKS_KEY));
    let notice = app.chrome.notice_text();
    assert_eq!(notice, Some("stopping sleep 30"));
    assert!(
        app.host.jobs.long_jobs().is_empty(),
        "too new to mention yet"
    );
}

#[test]
fn test_an_editor_chord_stays_on_the_editor_and_returns_to_it_from_elsewhere() {
    // The chord that opened an editor does nothing while the editor is in
    // front, and from another tab goes to the same editor again, which keeps
    // what it was showing rather than starting over.
    let mut app = App::new();
    let terminal = app.layout().focused();
    app.open_keys_editor();
    let keys = app.layout().focused();
    assert_ne!(keys, terminal);
    app.open_keys_editor();
    assert_eq!(app.layout().focused(), keys, "stays on the editor");
    app.show_tab(terminal);
    app.open_keys_editor();
    assert_eq!(app.layout().focused(), keys, "and to the same tab again");
    assert_eq!(app.open_tabs.pages().count(), 1, "never a second one");
    assert!(app.open_tabs.buried().is_empty(), "nothing was buried");
}

#[test]
fn test_f1_h_lists_the_focused_editor_s_own_keys() {
    let mut app = App::new();
    let tab = app.show_page(EditorKind::Sys, Box::new(SysEditor::new()));
    app.open_tab_keys(tab);
    app.settle_picker();
    let view = app.chrome.picker().expect("the list opens").view();
    assert_eq!(view.title, "Keys: Resource Monitor");
    let refresh = view
        .rows
        .iter()
        .find(|row| row.row.label == "Refresh Now")
        .expect("the editor's own key is listed");
    assert_eq!(refresh.row.hint, "r / C-r / M-S-r");
}

#[test]
fn test_the_browser_chord_pressed_twice_puts_it_away() {
    let mut app = App::new();
    app.open_file_browser();
    assert!(app.chrome.picker().is_some());
    app.open_file_browser();
    assert!(
        app.chrome.picker().is_none(),
        "the same chord toggles it off"
    );
}

#[test]
fn test_a_super_chord_and_a_shifted_enter_are_not_the_plain_keys_to_an_editor() {
    let (mut app, pane) = app_with_open_editor();
    let super_x = Key {
        sup: true,
        ..press(KeyCode::Char('x'))
    };
    assert!(!app.offer_key_to_editor(pane, &super_x), "s-x is not x");
    let shifted_enter = Key {
        shift: true,
        ..press(KeyCode::Enter)
    };
    assert!(!app.offer_key_to_editor(pane, &shifted_enter));
    assert!(app.offer_key_to_editor(pane, &press(KeyCode::Char('x'))));
}

#[test]
fn test_declined_key_falls_through_to_the_host() {
    let (mut app, pane) = app_with_open_editor();
    assert!(app.offer_key_to_editor(pane, &press(KeyCode::Char('x'))));
    assert!(!app.offer_key_to_editor(pane, &press(KeyCode::Char('z'))));
}

#[test]
fn test_a_pane_with_no_editor_is_never_offered_an_editor_key() {
    // Keys for an uncovered pane must reach its terminal, so the lookup has
    // to miss rather than find some other pane's editor.
    let (mut app, covered) = app_with_open_editor();
    let other = TabId(covered.0 + 1);
    assert!(!app.offer_key_to_editor(other, &press(KeyCode::Char('x'))));
}

#[test]
fn test_alt_z_toggles_wrapping_without_reaching_the_editor() {
    // Wrapping belongs to the pane rather than any one editor, so the host
    // spends the toggle itself: the key is claimed rather than declined,
    // which would otherwise hand it to the ordinary keymap.
    let (mut app, pane) = app_with_open_editor();
    let mut key = press(KeyCode::Char('z'));
    key.alt = true;
    let wraps = |app: &App| app.open_tabs.page(&pane).expect("the editor").view.wrap;
    assert!(wraps(&app), "wrapping is on to begin with");
    assert!(app.offer_key_to_editor(pane, &key), "the toggle is spent");
    assert!(!wraps(&app), "for this tab");
    assert!(app.offer_key_to_editor(pane, &key));
    assert!(wraps(&app));
}

#[test]
fn test_alt_z_in_a_text_editor_toggles_its_own_wrap_and_leaves_the_panes() {
    // The text editor wraps its own text; the pane's wrap does nothing to
    // it, so the toggle went nowhere while the host spent it first.
    let dir = std::env::temp_dir().join(format!("winter-alt-z-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("long.txt");
    std::fs::write(&file, "x".repeat(400)).expect("file");
    let mut app = App::new();
    let editor = winter_text::TextEditor::new(file, None).expect("opens");
    let tab = app.show_editor(EditorKind::Text, Box::new(editor));
    let mut key = press(KeyCode::Char('z'));
    key.alt = true;
    let pane_wraps = |app: &App| app.open_tabs.page(&tab).expect("the editor").view.wrap;
    let before = pane_wraps(&app);
    assert!(
        app.offer_key_to_editor(tab, &key),
        "the text editor takes it"
    );
    assert_eq!(pane_wraps(&app), before, "the pane's wrap is left alone");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_key_the_editor_takes_over_a_selection_acts_on_its_rows_and_ends_it() {
    let heard = Rc::new(RefCell::new(Vec::new()));
    let mut app = App::new();
    let tab = app.show_editor(
        EditorKind::Git,
        Box::new(SelectingEditor(Rc::clone(&heard))),
    );
    app.start_editor_cursor(tab);
    assert!(app.handle_editor_cursor_key(&press(KeyCode::Char('s'))));
    assert_eq!(heard.borrow().as_slice(), [RangeInclusive::new(0, 0)]);
    assert!(app.host.editor_cursor.is_none(), "the selection is over");
}

#[test]
fn test_an_editor_the_keyboard_leaves_for_another_pane_saves_and_keeps_its_mode() {
    let (mut app, editing, beside, file) = app_editing_beside_another("save-pane");
    let mode = mode_of(&mut app, editing);
    assert_eq!(
        std::fs::read_to_string(&file).expect("read"),
        "one\n",
        "not saved while typing"
    );
    app.set_focus(beside);
    assert_eq!(
        std::fs::read_to_string(&file).expect("read"),
        "zone\n",
        "saved on leaving"
    );
    assert!(
        !app.open_tabs
            .page_mut(&editing)
            .expect("an editor")
            .page
            .as_editor()
            .is_some_and(|editor| editor.is_dirty())
    );
    assert_eq!(
        mode_of(&mut app, editing),
        mode,
        "still in the mode it was typed in"
    );
    let _ = std::fs::remove_dir_all(file.parent().expect("dir"));
}

#[test]
fn test_an_editor_the_keyboard_comes_back_to_is_in_normal_mode() {
    use winter_model::modal::EditMode;

    let (mut app, editing, beside, file) = app_editing_beside_another("normal-back");
    assert_eq!(mode_of(&mut app, editing), Some(EditMode::Insert));
    app.set_focus(beside);
    app.set_focus(editing);
    assert_eq!(
        mode_of(&mut app, editing),
        Some(EditMode::Normal),
        "back from a pane"
    );
    app.open_tabs
        .page_mut(&editing)
        .expect("an editor")
        .page
        .on_key(&press(KeyCode::Char('i')));
    app.input.focus.window = false;
    app.sync_focus_reports();
    app.input.focus.window = true;
    app.sync_focus_reports();
    assert_eq!(
        mode_of(&mut app, editing),
        Some(EditMode::Normal),
        "back from a window"
    );
    let _ = std::fs::remove_dir_all(file.parent().expect("dir"));
}

#[test]
fn test_an_editor_saves_when_the_window_loses_the_keyboard() {
    let (mut app, _, _, file) = app_editing_beside_another("save-window");
    app.input.focus.window = false;
    app.sync_focus_reports();
    assert_eq!(std::fs::read_to_string(&file).expect("read"), "zone\n");
    let _ = std::fs::remove_dir_all(file.parent().expect("dir"));
}

#[test]
fn test_a_file_changed_on_disk_is_not_saved_over_on_leaving() {
    let (mut app, editing, beside, file) = app_editing_beside_another("save-stale");
    // Something else writes the file; the stamp moves on.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(&file, "theirs, longer\n").expect("write");
    app.set_focus(beside);
    assert_eq!(
        std::fs::read_to_string(&file).expect("read"),
        "theirs, longer\n",
        "left for a save that asks"
    );
    assert!(
        app.open_tabs
            .page_mut(&editing)
            .expect("an editor")
            .page
            .as_editor()
            .is_some_and(|editor| editor.is_dirty())
    );
    let _ = std::fs::remove_dir_all(file.parent().expect("dir"));
}

#[test]
fn test_a_deadline_an_editor_calls_off_is_no_longer_waited_on() {
    let (waited, _, later) = waited_on_after_deferring(true);
    assert_eq!(waited, Some(later), "the called-off deadline is dropped");
    let (waited, first, _) = waited_on_after_deferring(false);
    assert_eq!(waited, Some(first), "one not called off is kept, as before");
}

#[test]
fn test_ctrl_shift_k_in_a_text_editor_deletes_the_line_and_keeps_the_tab() {
    let dir = std::env::temp_dir().join(format!("winter-delete-lines-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").expect("file");
    let mut app = App::new();
    let editor = winter_text::TextEditor::new(file, None).expect("opens");
    let tab = app.show_editor(EditorKind::Text, Box::new(editor));
    let chord = Key {
        ctrl: true,
        shift: true,
        ..press(KeyCode::Char('k'))
    };
    app.route_key(true, chord);
    app.route_key(false, chord);
    let text = app
        .open_tabs
        .page(&tab)
        .and_then(|slot| slot.page.as_editor())
        .and_then(|editor| editor.document())
        .map(|(_, text)| text);
    assert_eq!(text.as_deref(), Some("two\nthree"), "the line, not the tab");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_ctrl_backspace_deletes_a_word_in_insert_mode_through_the_window() {
    let dir = std::env::temp_dir().join(format!("winter-c-bs-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("cbs.txt");
    std::fs::write(&file, "one two three\n").expect("file");
    let mut app = App::new();
    let editor = winter_text::TextEditor::new(file, None).expect("opens");
    let tab = app.show_editor(EditorKind::Text, Box::new(editor));
    // Enter Insert mode by typing, then kill a word back with C-Backspace.
    app.route_key(true, Key::plain(KeyCode::Char('A')));
    app.route_key(true, Key::with_ctrl(KeyCode::Backspace));
    let text = app
        .open_tabs
        .page(&tab)
        .and_then(|slot| slot.page.as_editor())
        .and_then(|editor| editor.document())
        .map(|(_, text)| text);
    // `A` appends at the end of the line, so the text is "one two threeA\n";
    // killing a word back takes "threeA" with the newline it ran into.
    assert_eq!(text.as_deref(), Some("one two "), "the word before went");
    let _ = std::fs::remove_dir_all(&dir);
}
