//! The window's editor tests: buffers: burying, views, and the buffer list.

use super::*;

#[test]
fn test_closing_an_editor_tab_buries_its_buffer_as_it_was() {
    // Closing loses nothing: the buffer is kept, state and all, and comes
    // back under the same tab. A rebuilt editor would come back at zero.
    let (mut app, tab) = app_with_open_editor();
    tally(&mut app, tab, 3);
    app.close_editor(tab);
    assert!(!app.layout().contains(tab), "no pane shows it");
    assert_eq!(app.open_tabs.buried(), [tab]);
    assert!(app.chrome.close_prompt().is_none(), "and nothing was asked");

    app.choose_buffer_row(&format!("buffer:{}", tab.0));
    assert_eq!(app.layout().focused(), tab, "back, under the tab it had");
    assert_eq!(showing(&app).as_deref(), Some("Counting 3"));
    assert!(app.open_tabs.buried().is_empty());
}

#[test]
fn test_kill_buffer_lets_a_buried_buffer_go() {
    let (mut app, tab) = app_with_open_editor();
    app.close_editor(tab);
    app.kill_buffer(tab);
    assert!(!app.open_tabs.is_editor(&tab));
    assert!(app.open_tabs.buried().is_empty());
}

#[test]
fn test_a_buried_buffer_is_ticked_only_for_work_under_way() {
    let (_, _, idle) = ticks_after(false, |app, tab| app.close_editor(tab));
    assert_eq!(*idle.borrow(), 0, "a buried buffer does nothing on a timer");
    // A build or a language server's flush goes on with no pane showing it.
    let (_, _, busy) = ticks_after(true, |app, tab| app.close_editor(tab));
    assert!(*busy.borrow() > 0, "work under way is seen through");
}

#[test]
fn test_a_buffer_shown_again_is_ticked_again() {
    let (mut app, tab, ticks) = ticks_after(false, |app, tab| app.close_editor(tab));
    app.show_buffer(tab);
    let end = Instant::now() + TICK_WAIT;
    while *ticks.borrow() == 0 && Instant::now() < end {
        app.tick_editors(Instant::now());
        std::thread::sleep(TICK_STEP);
    }
    assert!(*ticks.borrow() > 0, "catching up once a pane shows it");
}

#[test]
fn test_a_buried_buffer_is_told_so_and_told_again_when_shown() {
    let heard = Rc::new(RefCell::new(Vec::new()));
    let mut app = App::new();
    let tab = app.show_editor(EditorKind::Proc, Box::new(ShownEditor(Rc::clone(&heard))));
    app.close_editor(tab);
    assert_eq!(heard.borrow().as_slice(), ["hidden", "shed"]);
    app.show_buffer(tab);
    assert_eq!(heard.borrow().as_slice(), ["hidden", "shed", "shown"]);
}

#[test]
fn test_closing_a_terminal_tab_ends_it_with_nothing_buried() {
    let (mut app, _) = app_with_open_editor();
    let terminal = terminal_tab(&mut app, false);
    app.close_tab(terminal);
    assert!(app.open_tabs.terminal(&terminal).is_none(), "the shell ended");
    assert!(app.open_tabs.buried().is_empty());
}

#[test]
fn test_the_buffer_list_reads_terminals_then_buried_newest_first_then_shown() {
    let mut app = App::new();
    let terminal = app.layout().focused();
    app.open_tabs.insert_terminal(terminal, crate::window::test_support::idle_terminal());
    let older = app.show_editor(EditorKind::Grep, Box::new(CountingEditor::default()));
    let newer = app.show_editor(EditorKind::Sys, Box::new(CountingEditor::default()));
    let shown = app.show_editor(EditorKind::Proc, Box::new(CountingEditor::default()));
    app.close_editor(older);
    app.close_editor(newer);

    app.open_buffer_list();
    assert_eq!(app.chrome.picker().unwrap().mode, PickerMode::Buffers);
    assert_eq!(
        row_actions(&app),
        [
            format!("tab:{}", terminal.0),
            format!("buffer:{}", newer.0),
            format!("buffer:{}", older.0),
            format!("buffer:{}", shown.0),
        ]
    );
}

#[test]
fn test_the_buffer_list_opens_a_buffer_another_pane_shows_as_a_view_here() {
    let (mut app, there, sibling) = editor_and_another_pane(Box::new(CountingEditor::default()));
    app.choose_buffer_row(&format!("buffer:{}", there.0));
    let here = app.layout().focused();
    assert_ne!(here, there, "a tab of its own");
    assert_eq!(app.layout().tabs_in_pane(sibling), [sibling, here], "in the focused pane");
    assert_eq!(app.open_tabs.views_of(&here), [there, here], "on the same buffer");

    app.layout_mut().focus(sibling);
    app.choose_buffer_row(&format!("buffer:{}", there.0));
    assert_eq!(app.layout().focused(), here, "and never a second one in a pane");
}

#[test]
fn test_each_view_keeps_its_own_cursor_through_keys_on_the_other() {
    let (mut app, there, _) = editor_and_another_pane(Box::new(ViewedEditor::default()));
    app.show_buffer(there);
    let here = app.layout().focused();
    tally(&mut app, here, 2);
    assert_eq!(title_in(&mut app, here), "Viewed 2");
    assert_eq!(title_in(&mut app, there), "Viewed 0", "the other view stayed put");
    tally(&mut app, there, 1);
    assert_eq!(title_in(&mut app, here), "Viewed 2");
    assert_eq!(title_in(&mut app, there), "Viewed 1");
}

#[test]
fn test_views_of_a_kind_with_no_view_state_share_one() {
    let (mut app, there, _) = editor_and_another_pane(Box::new(CountingEditor::default()));
    app.show_buffer(there);
    let here = app.layout().focused();
    tally(&mut app, here, 2);
    assert_eq!(title_in(&mut app, there), "Counting 2", "one editor, one state");
}

#[test]
fn test_closing_one_view_leaves_the_buffer_to_the_other() {
    let (mut app, there, _) = editor_and_another_pane(Box::new(UnsavedEditor("main.rs")));
    app.show_buffer(there);
    let here = app.layout().focused();
    app.close_tab(here);
    assert!(app.chrome.close_prompt().is_none(), "nothing is lost");
    assert!(app.open_tabs.buried().is_empty(), "nor buried: it is still shown");
    assert_eq!(app.open_tabs.views_of(&there), [there]);
}

#[test]
fn test_killing_a_buffer_closes_every_view_of_it() {
    let (mut app, there, _) = editor_and_another_pane(Box::new(CountingEditor::default()));
    app.show_buffer(there);
    let here = app.layout().focused();
    app.kill_buffer(here);
    assert!(!app.open_tabs.is_editor(&here));
    assert!(!app.open_tabs.is_editor(&there));
    assert!(!app.layout().contains(there));
}

#[test]
fn test_moving_a_tab_into_a_pane_with_its_buffer_closes_the_moved_tab() {
    let (mut app, there, sibling) = editor_and_another_pane(Box::new(CountingEditor::default()));
    app.show_buffer(there);
    let here = app.layout().focused();
    app.move_tab_to_pane(here, there);
    assert!(!app.layout().contains(here), "the moved tab closed");
    assert_eq!(app.layout().focused(), there, "for the one already there");
    assert_eq!(app.layout().tabs_in_pane(sibling), [sibling]);
}

#[test]
fn test_a_buffer_shown_twice_is_ticked_once() {
    let ticks = Rc::new(RefCell::new(0));
    let editor = PollingEditor {
        after: Duration::ZERO,
        busy: false,
        ticks: Rc::clone(&ticks),
    };
    let (mut app, there, _) = editor_and_another_pane(Box::new(editor));
    app.show_buffer(there);
    let now = Instant::now();
    app.tick_editors(now);
    app.tick_editors(now + Duration::from_secs(1));
    assert_eq!(*ticks.borrow(), 1);
}

#[test]
fn test_every_buffer_list_row_draws_the_icon_its_tab_would() {
    // A buried buffer has no tab in a strip, so its row's icon comes from
    // its kind, not from whatever is open now.
    let mut app = App::new();
    app.show_editor(EditorKind::Grep, Box::new(CountingEditor::default()));
    let buried = app.show_editor(EditorKind::Git, Box::new(CountingEditor::default()));
    app.close_editor(buried);

    app.open_buffer_list();
    let palette = app.chrome.picker().expect("the list opens");
    let icons: Vec<Option<&IconKind>> = palette
        .entries
        .iter()
        .filter(|entry| entry.action.starts_with("buffer:"))
        .map(|entry| entry.icon.as_ref())
        .collect();
    assert_eq!(
        icons,
        [
            Some(&IconKind::Editor { kind: "git" }),
            Some(&IconKind::Editor { kind: "grep" }),
        ]
    );
}

#[test]
fn test_browsing_onto_parent_dir_item_changes_path_instead_of_opening_dir_view() {
    use winit::keyboard::NamedKey;

    let tree = TempTree::new("parent-dir", &["child/sub/"]);
    let mut app = App::new();
    let child_dir = tree.0.join("child");
    app.open_picker(crate::model::picker::Picker::open_files(child_dir.clone()));

    // Press Enter on "../"
    browse_to(&mut app, "../");
    browse_key(&mut app, NamedKey::Enter);

    // Picker stays open and path changes to parent directory
    let palette = app.chrome.picker().expect("the browser is still open");
    assert_eq!(
        palette.dir.as_ref(),
        Some(&tree.0),
        "path changed to parent dir item"
    );

    // Whereas pressing Enter on a directory item (like "child/") opens in a dir view
    browse_to(&mut app, "child/");
    browse_key(&mut app, NamedKey::Enter);

    assert!(app.chrome.picker().is_none(), "the browser is done");
    let tab = app.layout().focused();
    assert_eq!(
        app.open_tabs.editor(&tab).map(|slot| slot.kind),
        Some(EditorKind::Dir),
        "and the directory is open in Dir editor"
    );
    let slot = app.open_tabs.editor(&tab).unwrap();
    assert_eq!(slot.editor.root_dir(), Some(child_dir));
}

#[test]
fn test_asking_for_the_status_again_reads_an_open_view_again() {
    // The view's watcher misses some changes, so asking for the status
    // is the way to see what it is now, from the view or from beside it.
    let (mut app, first, _) = two_panes();
    app.open_git_editor();
    let git = app.layout().focused();

    app.host.jobs.cancel_for(git);
    app.open_git_editor();
    assert_eq!(app.layout().focused(), git);
    assert!(app.host.jobs.is_busy_for(git), "asked from the view itself");

    app.host.jobs.cancel_for(git);
    app.layout_mut().focus(first);
    app.open_git_editor();
    assert_eq!(app.layout().focused(), git);
    assert!(app.host.jobs.is_busy_for(git), "asked from the pane beside it");
}

#[test]
fn test_a_server_edit_to_a_file_not_open_lands_in_a_buried_buffer_of_it() {
    let tree = TempTree::new("foreign-edit", &["one.txt", "two.txt"]);
    let mut app = App::new();
    app.open_text_editor(OpenTarget::file(tree.0.join("one.txt")));
    let edit = winter_lsp::protocol::TextEdit {
        new_text: "edited".to_string(),
        range: winter_lsp::protocol::Range::default(),
    };
    let two = tree.0.join("two.txt");
    app.edit_file(&two, &[edit]);
    let buried = app.open_tabs.buried().to_vec();
    assert_eq!(buried.len(), 1, "opened, buried");
    let slot = app.open_tabs.editor(&buried[0]).expect("a buffer");
    assert!(slot.editor.holds_file(&two));
    assert!(slot.editor.is_dirty(), "edited, not saved");
}

#[test]
fn test_a_buried_file_is_saved_buried_and_a_buried_listing_is_not() {
    let dir = std::env::temp_dir().join(format!("winter-save-buried-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("notes.md");
    std::fs::write(&file, "one\n").expect("write");
    let mut app = App::new();
    let listing = app.show_editor(EditorKind::Dir, Box::new(DirEditor::new(dir.clone())));
    app.open_path_editor(OpenTarget::file(file.clone()));
    let text = app.layout().focused();
    app.close_editor(text);
    app.close_editor(listing);
    let saved = app.editor_sessions();
    assert_eq!(saved.len(), 1, "the file alone: {saved:?}");
    assert!(saved[0].buried);
    assert_eq!(saved[0].kind, EditorKind::Text.name());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_an_editor_is_ticked_once_the_deadline_it_asked_for_passes() {
    // An editor says when it next wants a tick in its own time, a moment
    // after the loop took its own: "at once" and "in a little while" must
    // still come due, rather than staying just ahead of the loop for ever.
    for after in [Duration::ZERO, Duration::from_millis(10)] {
        let ticks = Rc::new(RefCell::new(0));
        let mut app = App::new();
        let editor = PollingEditor {
            after,
            busy: false,
            ticks: Rc::clone(&ticks),
        };
        app.show_editor(EditorKind::Proc, Box::new(editor));
        let end = Instant::now() + TICK_WAIT;
        while *ticks.borrow() == 0 && Instant::now() < end {
            let wake = app.tick_editors(Instant::now());
            let pause = wake.map_or(TICK_STEP, |at| at.saturating_duration_since(Instant::now()));
            std::thread::sleep(pause.min(TICK_STEP));
        }
        assert!(
            *ticks.borrow() > 0,
            "an editor asking for a tick {after:?} out is ticked"
        );
    }
}

#[test]
fn test_opening_an_editor_whose_buffer_is_buried_brings_that_buffer_back() {
    let mut app = App::new();
    app.open_keys_editor();
    let keys = app.layout().focused();
    app.close_editor(keys);
    app.open_keys_editor();
    assert_eq!(app.layout().focused(), keys, "the buried one, back");
    assert_eq!(app.open_tabs.editors().count(), 1, "never a second one");
    assert!(app.open_tabs.buried().is_empty());
}

#[test]
fn test_listing_a_directory_a_buried_listing_shows_brings_it_back() {
    let dir = std::env::temp_dir().join(format!("winter-buried-dir-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut app = App::new();
    app.open_dir_editor_at(dir.clone());
    let listing = app.layout().focused();
    app.close_editor(listing);
    app.open_dir_editor_at(dir.clone());
    assert_eq!(app.layout().focused(), listing, "the buried listing, back");
    assert_eq!(app.open_tabs.editors().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_picture_opens_in_the_viewer_an_svg_included_and_beside_a_tab_too() {
    let dir = std::env::temp_dir().join(format!("winter-open-svg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let svg = dir.join("logo.svg");
    std::fs::write(&svg, "<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'/>").expect("svg");
    let kind_shown = |app: &App| app.open_tabs.editor(&app.layout().focused()).map(|slot| slot.kind());

    let mut app = App::new();
    app.open_path_editor(OpenTarget::file(svg.clone()));
    assert_eq!(kind_shown(&app), Some(EditorKind::Image), "drawn, not its markup");

    // Beside the tab it was opened from, the picture is still drawn.
    let mut app = App::new();
    let from = app.layout().focused();
    app.open_beside(from, OpenTarget::file(svg));
    assert_eq!(kind_shown(&app), Some(EditorKind::Image));
    assert!(app.layout().shown_tabs().contains(&from), "the tab it came from stays in sight");
    let _ = std::fs::remove_dir_all(&dir);
}
