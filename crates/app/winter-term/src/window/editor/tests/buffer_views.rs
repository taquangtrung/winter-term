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

    app.show_buffer(tab);
    assert_eq!(app.layout().focused(), tab, "back, under the tab it had");
    assert_eq!(showing(&app).as_deref(), Some("Counting 3"));
    assert!(app.open_tabs.buried().is_empty());
}

#[test]
fn test_reopening_brings_back_the_tab_closed_last_as_it_was() {
    let (mut app, tab) = app_with_open_editor();
    tally(&mut app, tab, 3);
    app.close_editor(tab);
    app.reopen_closed_tab();
    assert_eq!(app.layout().focused(), tab, "back, under the tab it had");
    assert_eq!(
        showing(&app).as_deref(),
        Some("Counting 3"),
        "as it was left"
    );
}

#[test]
fn test_reopening_passes_over_a_closed_buffer_since_killed() {
    let (mut app, tab) = app_with_open_editor();
    let other = app.show_editor(EditorKind::Grep, Box::new(CountingEditor::default()));
    app.close_editor(tab);
    app.close_editor(other);
    app.kill_buffer(other);
    app.reopen_closed_tab();
    assert_eq!(app.layout().focused(), tab, "the one closed before it");
}

#[test]
fn test_reopening_with_nothing_closed_says_so() {
    let mut app = App::new();
    app.reopen_closed_tab();
    let said = crate::window::layout::tabs::NOTHING_CLOSED;
    assert_eq!(app.chrome.notice_text(), Some(said));
}

#[test]
fn test_a_closed_terminal_is_kept_to_reopen_where_it_stood() {
    // Its program is gone, so it comes back as a new shell in the directory
    // the old one was in.
    let (mut app, _) = app_with_open_editor();
    let terminal = terminal_tab(&mut app, false);
    app.close_tab(terminal);
    assert!(matches!(
        app.tabs.closed.last(),
        Some(crate::window::layout::tabs::ClosedTab::Terminal(_))
    ));
}

#[test]
fn test_the_recently_used_walk_of_a_kind_skips_tabs_of_other_kinds() {
    let mut app = App::new();
    let first = app.show_editor(EditorKind::Grep, Box::new(CountingEditor::default()));
    let other = app.show_editor(EditorKind::Sys, Box::new(CountingEditor::default()));
    let last = app.show_editor(EditorKind::Grep, Box::new(CountingEditor::default()));
    assert_eq!(app.layout().focused(), last);
    app.recently_used_tab_of_kind(true);
    assert_eq!(
        app.layout().focused(),
        first,
        "past the tab of another kind"
    );
    let walked = |app: &App| app.tabs.recent_walk.as_ref().map(|walk| walk.order.clone());
    assert!(
        !walked(&app).unwrap_or_default().contains(&other),
        "it walks only its kind"
    );
    app.recently_used_tab_of_kind(false);
    assert_eq!(app.layout().focused(), last, "and back");
    app.recently_used_tab(true);
    assert!(
        walked(&app).unwrap_or_default().contains(&other),
        "the walk of every kind, started anew, takes every tab"
    );
}

#[test]
fn test_kill_buffer_lets_a_buried_buffer_go() {
    let (mut app, tab) = app_with_open_editor();
    app.close_editor(tab);
    app.kill_buffer(tab);
    assert!(!app.open_tabs.is_page(&tab));
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
    assert!(
        app.open_tabs.terminal(&terminal).is_none(),
        "the shell ended"
    );
    assert!(app.open_tabs.buried().is_empty());
}

#[test]
fn test_the_buffer_list_reads_terminals_then_buried_newest_first_then_shown() {
    let mut app = App::new();
    let terminal = app.layout().focused();
    app.open_tabs
        .insert_terminal(terminal, crate::window::test_support::idle_terminal());
    let older = app.show_editor(EditorKind::Grep, Box::new(CountingEditor::default()));
    let newer = app.show_editor(EditorKind::Sys, Box::new(CountingEditor::default()));
    let shown = app.show_editor(EditorKind::Proc, Box::new(CountingEditor::default()));
    app.close_editor(older);
    app.close_editor(newer);

    let mut listing = app.buffer_listing();
    assert_eq!(
        row_actions(&mut listing),
        [
            Some(WindowOutcome::FocusTab(terminal)),
            Some(WindowOutcome::ShowBuffer(newer)),
            Some(WindowOutcome::ShowBuffer(older)),
            Some(WindowOutcome::ShowBuffer(shown)),
        ]
    );
}

#[test]
fn test_the_buffer_list_opens_a_buffer_another_pane_shows_as_a_view_here() {
    let (mut app, there, sibling) = editor_and_another_pane(Box::new(CountingEditor::default()));
    app.show_buffer(there);
    let here = app.layout().focused();
    assert_ne!(here, there, "a tab of its own");
    assert_eq!(
        app.layout().tabs_in_pane(sibling),
        [sibling, here],
        "in the focused pane"
    );
    assert_eq!(
        app.open_tabs.views_of(&here),
        [there, here],
        "on the same buffer"
    );

    app.layout_mut().focus(sibling);
    app.show_buffer(there);
    assert_eq!(
        app.layout().focused(),
        here,
        "and never a second one in a pane"
    );
}

#[test]
fn test_each_view_keeps_its_own_cursor_through_keys_on_the_other() {
    let (mut app, there, _) = editor_and_another_pane(Box::new(ViewedEditor::default()));
    app.show_buffer(there);
    let here = app.layout().focused();
    tally(&mut app, here, 2);
    assert_eq!(title_in(&mut app, here), "Viewed 2");
    assert_eq!(
        title_in(&mut app, there),
        "Viewed 0",
        "the other view stayed put"
    );
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
    assert_eq!(
        title_in(&mut app, there),
        "Counting 2",
        "one editor, one state"
    );
}

#[test]
fn test_closing_one_view_leaves_the_buffer_to_the_other() {
    let (mut app, there, _) = editor_and_another_pane(Box::new(UnsavedEditor("main.rs")));
    app.show_buffer(there);
    let here = app.layout().focused();
    app.close_tab(here);
    assert!(app.chrome.close_prompt().is_none(), "nothing is lost");
    assert!(
        app.open_tabs.buried().is_empty(),
        "nor buried: it is still shown"
    );
    assert_eq!(app.open_tabs.views_of(&there), [there]);
}

#[test]
fn test_killing_a_buffer_closes_every_view_of_it() {
    let (mut app, there, _) = editor_and_another_pane(Box::new(CountingEditor::default()));
    app.show_buffer(there);
    let here = app.layout().focused();
    app.kill_buffer(here);
    assert!(!app.open_tabs.is_page(&here));
    assert!(!app.open_tabs.is_page(&there));
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

    let mut listing = app.buffer_listing();
    let winter_picker::Feed::Fixed(rows) = listing.feed("") else {
        panic!("a listing gives its rows at once");
    };
    let buffers: Vec<Option<IconKind>> = rows
        .into_iter()
        .filter(|row| {
            matches!(
                listing.run(CHOOSE.id, row.key, ""),
                Some(WindowOutcome::ShowBuffer(_))
            )
        })
        .map(|row| row.icon)
        .collect();
    assert_eq!(
        buffers,
        [
            Some(IconKind::Editor { kind: "git" }),
            Some(IconKind::Editor { kind: "grep" }),
        ]
    );
}

#[test]
fn test_browsing_onto_parent_dir_item_changes_path_instead_of_opening_dir_view() {
    let tree = TempTree::new("parent-dir", &["child/sub/"]);
    let mut app = App::new();
    let child_dir = tree.0.join("child");
    app.open_browser_at(child_dir.clone());

    // Press Enter on "../"
    browse_to(&mut app, "../");
    browse_key(&mut app, "RET");

    // Picker stays open and path changes to parent directory
    app.settle_picker();
    let palette = app.chrome.picker().expect("the browser is still open");
    assert_eq!(
        palette.view().title,
        winter_model::path::abbreviate_home(&tree.0),
        "path changed to parent dir item"
    );

    // Whereas pressing Enter on a directory item (like "child/") opens in a dir view
    browse_to(&mut app, "child/");
    browse_key(&mut app, "RET");

    assert!(app.chrome.picker().is_none(), "the browser is done");
    let tab = app.layout().focused();
    assert_eq!(
        app.open_tabs.page(&tab).map(|slot| slot.kind),
        Some(EditorKind::Dir),
        "and the directory is open in Dir editor"
    );
    let slot = app.open_tabs.page(&tab).unwrap();
    assert_eq!(
        slot.page.as_editor().and_then(|editor| editor.root_dir()),
        Some(child_dir)
    );
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
    assert!(
        app.host.jobs.is_busy_for(git),
        "asked from the pane beside it"
    );
}

#[test]
fn test_a_server_edit_to_a_file_not_open_lands_in_a_buried_buffer_of_it() {
    let tree = TempTree::new("foreign-edit", &["one.txt", "two.txt"]);
    let mut app = App::new();
    app.open_text_editor(OpenTarget::file(tree.0.join("one.txt")));
    let edit = winter_model::editor::FileEdit {
        new_text: "edited".to_string(),
        ..Default::default()
    };
    let two = tree.0.join("two.txt");
    app.edit_file(&two, &[edit]);
    let buried = app.open_tabs.buried().to_vec();
    assert_eq!(buried.len(), 1, "opened, buried");
    let slot = app.open_tabs.page(&buried[0]).expect("a buffer");
    assert!(slot
        .page
        .as_editor()
        .is_some_and(|editor| editor.holds_file(&two)));
    assert!(
        slot.page
            .as_editor()
            .is_some_and(|editor| editor.is_dirty()),
        "edited, not saved"
    );
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
        app.show_page(EditorKind::Proc, Box::new(editor));
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
fn test_a_page_is_one_per_window_and_closing_it_buries_nothing() {
    let mut app = App::new();
    app.open_keys_editor();
    let keys = app.layout().focused();
    app.split_pane(winter_model::layout::Direction::Vertical);
    app.open_keys_editor();
    assert_eq!(
        app.layout().focused(),
        keys,
        "the window's one, from another pane"
    );
    let keys_pages = |app: &App| {
        app.open_tabs
            .pages()
            .filter(|(_, slot)| slot.kind == EditorKind::Keys)
            .count()
    };
    assert_eq!(keys_pages(&app), 1, "never a second one");

    app.close_editor(keys);
    assert!(!app.open_tabs.is_page(&keys), "let go, not buried");
    assert!(app.open_tabs.buried().is_empty());
    app.open_keys_editor();
    assert_ne!(app.layout().focused(), keys, "a fresh one");
    assert_eq!(keys_pages(&app), 1);
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
    assert_eq!(app.open_tabs.pages().count(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_picture_opens_in_the_viewer_an_svg_included_and_beside_a_tab_too() {
    let dir = std::env::temp_dir().join(format!("winter-open-svg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let svg = dir.join("logo.svg");
    std::fs::write(
        &svg,
        "<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'/>",
    )
    .expect("svg");
    let kind_shown = |app: &App| {
        app.open_tabs
            .page(&app.layout().focused())
            .map(|slot| slot.kind())
    };

    let mut app = App::new();
    app.open_path_editor(OpenTarget::file(svg.clone()));
    assert_eq!(
        kind_shown(&app),
        Some(EditorKind::Image),
        "drawn, not its markup"
    );

    // Beside the tab it was opened from, the picture is still drawn.
    let mut app = App::new();
    let from = app.layout().focused();
    app.open_beside(from, OpenTarget::file(svg));
    assert_eq!(kind_shown(&app), Some(EditorKind::Image));
    assert!(
        app.layout().shown_tabs().contains(&from),
        "the tab it came from stays in sight"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_file_s_tab_is_listed_by_its_full_path_in_the_buffer_and_recent_lists() {
    let dir = std::env::temp_dir().join(format!("winter-full-path-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "text\n").expect("file");
    let mut app = App::new();
    let editor = winter_text::TextEditor::new(file.clone(), None).expect("opens");
    let tab = app.show_editor(EditorKind::Text, Box::new(editor));
    let full = file.display().to_string();
    let label_of = |mut listing: Listing, outcome: WindowOutcome| {
        let winter_picker::Feed::Fixed(rows) = listing.feed("") else {
            panic!("a listing gives its rows at once");
        };
        rows.into_iter()
            .find(|row| listing.run(CHOOSE.id, row.key, "") == Some(outcome.clone()))
            .map(|row| row.label)
    };

    let buffers = app.buffer_listing();
    assert_eq!(
        label_of(buffers, WindowOutcome::ShowBuffer(tab)),
        Some(full.clone()),
        "not its name and folder apart"
    );
    let recent = app.recent_listing();
    assert_eq!(
        label_of(recent, WindowOutcome::FocusTab(tab)),
        Some(full),
        "not its tab's title"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_recent_file_open_in_a_pane_is_listed_once_as_its_tab() {
    let dir = std::env::temp_dir().join(format!("winter-recent-dedup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "text\n").expect("file");
    let mut app = App::new();
    let editor = winter_text::TextEditor::new(file.clone(), None).expect("opens");
    let tab = app.show_editor(EditorKind::Text, Box::new(editor));
    app.recent.files.insert(0, file.clone());

    let mut recent = app.recent_listing();
    let winter_picker::Feed::Fixed(rows) = recent.feed("") else {
        panic!("a listing gives its rows at once");
    };
    let outcomes: Vec<WindowOutcome> = rows
        .into_iter()
        .filter_map(|row| recent.run(CHOOSE.id, row.key, ""))
        .collect();
    assert!(outcomes.contains(&WindowOutcome::FocusTab(tab)));
    assert!(
        !outcomes
            .iter()
            .any(|outcome| matches!(outcome, WindowOutcome::OpenFile(at) if at.path == file)),
        "the file is not offered again below its tab"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_an_open_page_is_saved_by_kind_and_built_afresh_on_restore() {
    let mut app = App::new();
    app.open_keys_editor();
    let keys = app.layout().focused();
    let saved = app.editor_sessions();
    let page = saved
        .iter()
        .find(|session| session.id == keys.0 as usize)
        .expect("saved");
    assert_eq!((page.kind.as_str(), page.path.as_str()), ("keys", ""));

    let mut restored = App::new();
    let id = winter_model::layout::TabId(99);
    assert!(restored.restore_editor(id, page));
    let slot = restored.open_tabs.page(&id).expect("built");
    assert_eq!((slot.kind, slot.buffer), (EditorKind::Keys, None));
}

#[test]
fn test_stepping_by_kind_goes_to_the_pane_s_next_tab_of_the_same_kind_stopping_at_the_ends() {
    let mut app = App::new();
    let first = app.show_editor(EditorKind::Grep, Box::new(CountingEditor::default()));
    let listing = app.show_editor(EditorKind::Dir, Box::new(CountingEditor::default()));
    let second = app.show_editor(EditorKind::Grep, Box::new(CountingEditor::default()));
    app.cycle_tab_of_kind(true);
    assert_eq!(
        app.layout().focused(),
        second,
        "the last stays: no wrapping"
    );
    app.cycle_tab_of_kind(false);
    assert_eq!(app.layout().focused(), first, "the Dir between is skipped");
    app.cycle_tab_of_kind(false);
    assert_eq!(
        app.layout().focused(),
        first,
        "the first stays: no wrapping"
    );
    app.show_tab(listing);
    app.cycle_tab_of_kind(false);
    assert_eq!(
        app.layout().focused(),
        listing,
        "the only one of its kind stays"
    );
}
