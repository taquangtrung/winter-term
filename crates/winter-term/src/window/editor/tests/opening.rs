//! The window's editor tests: opening editors, files, and listings, and closing them again.

use super::*;

#[test]
fn test_an_editor_opens_where_the_editor_is_looking_not_where_the_shell_is() {
    // Opening the file browser while reading a file used to start at the
    // shell's directory, which for a file opened from elsewhere is not
    // even the same tree. The editor tab being looked at answers first.
    let mut app = App::new();
    let looking_at = PathBuf::from("/tmp/winter-test-somewhere-else");
    app.show_editor(EditorKind::Dir, Box::new(LocatedEditor(looking_at.clone())));
    assert_eq!(app.focused_start_dir(), looking_at);
}

#[test]
fn test_an_editor_tab_with_no_place_of_its_own_starts_where_its_shell_is() {
    // An editor that says nothing about where it is looking defers to the
    // terminal it was opened from, so a tab opened from it does not land
    // in the process's own directory.
    let mut app = App::new();
    let from_shell = app.focused_start_dir();
    app.show_editor(EditorKind::Grep, Box::new(CountingEditor::default()));
    assert_eq!(app.focused_start_dir(), from_shell);
}

#[test]
fn test_an_editor_opens_as_a_tab_beside_the_terminal_without_splitting() {
    let mut app = App::new();
    let terminal = app.layout().focused();
    let tab = app.show_editor(EditorKind::Grep, Box::new(CountingEditor::default()));
    assert_ne!(tab, terminal, "a tab of its own");
    assert_eq!(
        app.layout().shown_tabs(),
        vec![tab],
        "showing in the same pane"
    );
    assert_eq!(app.layout().members(), vec![terminal, tab]);
    assert!(app.open_tabs.is_editor(&tab));
}

#[test]
fn test_opening_a_file_opens_an_editor_tab_and_a_binary_one_its_bytes() {
    // The host side of every `Enter` on a file: the editor has to end up
    // in a tab, and a file it will not open has to leave the pane showing
    // whatever it was showing rather than an empty editor.
    let dir = std::env::temp_dir().join(format!("winter-open-editor-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let text = dir.join("a.txt");
    std::fs::write(&text, "one\ntwo\n").expect("temp file");
    let binary = dir.join("a.bin");
    std::fs::write(&binary, [0x00, 0x01]).expect("temp file");

    let mut app = App::new();
    let terminal = app.layout().focused();
    app.open_text_editor(OpenTarget::at_line(text, 2));
    let editor = app.layout().focused();
    assert_eq!(
        app.open_tabs.editor(&editor).map(|slot| slot.kind),
        Some(EditorKind::Text)
    );

    app.close_editor(editor);
    assert_eq!(app.layout().focused(), terminal);
    // A file that is no text opens as its bytes rather than as nothing.
    app.open_text_editor(OpenTarget::file(binary));
    let shown = app.layout().focused();
    assert_eq!(
        app.open_tabs.editor(&shown).map(|slot| slot.kind),
        Some(EditorKind::Hex)
    );
    app.close_editor(shown);
    assert_eq!(app.layout().focused(), terminal);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_opening_a_file_a_tab_in_the_pane_holds_goes_to_that_tab_at_the_line() {
    // `Enter` on a git hunk, a grep match, an error reference: a file the
    // pane already has open is shown where it is, at the line asked for,
    // rather than opened a second time.
    let dir = std::env::temp_dir().join(format!("winter-reveal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let text = dir.join("a.txt");
    std::fs::write(&text, "one\ntwo\nthree\n").expect("temp file");

    let mut app = App::new();
    let terminal = app.layout().focused();
    app.open_path_editor(OpenTarget::file(text.clone()));
    let editor = app.layout().focused();
    app.show_tab(terminal);

    app.open_path_editor(OpenTarget::at_line(text.clone(), 3));
    assert_eq!(app.layout().focused(), editor, "the tab holding it, shown");
    assert_eq!(app.open_tabs.editors().count(), 1, "not opened again");
    let shown = app
        .open_tabs
        .editor(&editor)
        .and_then(|slot| slot.editor.snapshot());
    assert_eq!(
        shown,
        Some(OpenTarget::at_line(text, 3)),
        "at the line asked for"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_closing_a_tab_opened_from_another_goes_back_to_it() {
    // Opening a file from a listing and closing it again has to land back
    // on the listing, not on whichever tab sits next to it on the strip.
    let mut app = App::new();
    let listing = app.show_editor(EditorKind::Grep, Box::new(CountingEditor::default()));
    app.layout_mut().focus(TabId(0));
    let other = app.show_editor(EditorKind::Proc, Box::new(CountingEditor::default()));
    app.show_tab(listing);
    let file = app.show_editor(EditorKind::Sys, Box::new(CountingEditor::default()));
    assert_eq!(
        app.layout().members(),
        vec![TabId(0), other, listing, file],
        "the file opens right beside the listing"
    );

    app.close_editor(file);
    assert_eq!(app.layout().focused(), listing);
    assert!(
        app.open_tabs.is_editor(&listing),
        "and the listing is as it was"
    );
}

#[test]
fn test_closing_an_editor_leaves_the_terminal_as_it_was() {
    // The terminal tab kept running beside the editor, so it has to come
    // back in the mode it was in.
    let mut app = App::new();
    let terminal = app.layout().focused();
    let shell = crate::terminal::session::Terminal::with_command(
        20,
        5,
        portable_pty::CommandBuilder::new("true"),
        1000,
    )
    .expect("test terminal spawn");
    app.open_tabs.insert_terminal(terminal, shell);
    app.open_tabs.set_mode(&terminal, Mode::Normal);
    let tab = app.show_editor(EditorKind::Grep, Box::new(CountingEditor::default()));

    app.close_editor(tab);
    assert_eq!(app.open_tabs.buried(), [tab]);
    assert_eq!(app.layout().members(), vec![terminal]);
    assert_eq!(app.open_tabs.mode(&terminal), Some(Mode::Normal));
}

#[test]
fn test_closing_a_tab_a_program_waits_on_lets_the_program_go() {
    // A buried buffer would keep `git commit` waiting for as long as the
    // window lives.
    let mut app = App::new();
    let tab = app.show_editor(EditorKind::Text, Box::new(WaitedEditor));
    app.close_tab_now(tab, Closing::Keep);
    assert!(!app.open_tabs.is_editor(&tab), "killed, not buried");
    assert!(app.open_tabs.buried().is_empty());
}

#[test]
fn test_two_tabs_on_one_file_keep_their_own_cursor() {
    let dir = std::env::temp_dir().join(format!("winter-text-views-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("notes.txt");
    std::fs::write(&file, "one\ntwo\nthree\n").expect("temp file");
    let editor = crate::editors::text::TextEditor::new(file, None).expect("opens");
    let (mut app, there, _) = editor_and_another_pane(Box::new(editor));
    app.show_buffer(there);
    let here = app.layout().focused();
    for _ in 0..2 {
        app.offer_key_to_editor(here, &press(KeyCode::Char('j')));
    }
    let at = |app: &mut App, tab: TabId| {
        let slot = app.open_tabs.editor_mut(&tab).expect("an editor");
        slot.editor.status().segments.first().cloned().unwrap_or_default()
    };
    assert_eq!(at(&mut app, here), "3:1");
    assert_eq!(at(&mut app, there), "1:1", "the other tab stayed put");
    assert_eq!(at(&mut app, here), "3:1", "and this one is back where it was");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_every_editor_kind_reads_back_from_the_name_a_session_stores() {
    // The names sit in one table written by hand, so two kinds given the
    // same name would bring the second back from a session as the first.
    for kind in EditorKind::ALL {
        assert_eq!(EditorKind::from_name(kind.name()), Some(kind), "{kind:?}");
    }
}

#[test]
fn test_every_editor_kind_but_the_editor_has_artwork_of_its_own() {
    // The icon table is keyed by the kind's name as a string, so a kind
    // renamed or added without a row there falls back to the generic file
    // icon in silence. The editor draws its file's icon instead.
    for kind in EditorKind::ALL {
        assert_eq!(
            crate::icons::name_for(&IconKind::Editor { kind: kind.name() })
                != crate::icons::name_for(&IconKind::Editor { kind: "" }),
            kind != EditorKind::Text,
            "{kind:?}"
        );
    }
}

#[test]
fn test_an_editor_report_reaches_the_status_bar_whatever_the_outcome() {
    // An editor reports beside another outcome, such as the job that re-reads
    // it, so the report cannot ride on a `Notice` outcome alone.
    let mut app = App::new();
    let editor = ReportingEditor(Some(Notice::info("pushed")));
    let tab = app.show_editor(EditorKind::Git, Box::new(editor));
    app.act_on_editor_outcome(tab, EditorOutcome::Consumed);
    assert_eq!(app.chrome.notice_text(), Some("pushed"));
}

#[test]
fn test_browsing_into_a_directory_keeps_the_palette_up_rooted_there() {
    use winit::keyboard::NamedKey;

    // Tab steps into a directory, keeping the browser up and rooted there.
    let tree = TempTree::new("step", &["src/", "README.md"]);
    std::fs::write(tree.0.join("src").join("lib.rs"), "fn main() {}\n").expect("nested file");
    let mut app = App::new();
    app.open_picker(crate::model::picker::Picker::open_files(tree.0.clone()));

    browse_to(&mut app, "src/");
    browse_key(&mut app, NamedKey::Tab);

    let palette = app.chrome.picker().expect("still browsing");
    assert_eq!(palette.dir.as_ref(), Some(&tree.0.join("src")));
    assert!(
        palette.entries.iter().any(|entry| entry.label == "lib.rs"),
        "showing what is in there"
    );

    // Tab on a file does not open it; the browser stays up.
    browse_to(&mut app, "lib.rs");
    browse_key(&mut app, NamedKey::Tab);
    assert!(app.chrome.picker().is_some(), "tab on a file keeps the browser up");

    // Backspace rubs out typed text and never leaves the directory, even
    // with nothing typed.
    browse_key(&mut app, NamedKey::Backspace);
    let palette = app.chrome.picker().expect("still browsing");
    assert_eq!(palette.dir.as_ref(), Some(&tree.0.join("src")));

    // `C-h` is the way back out.
    app.input.modifiers = winit::keyboard::ModifiersState::CONTROL.into();
    let tab = app.layout().focused();
    let phys =
        winit::keyboard::PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
    app.press_palette_key(&winit::keyboard::Key::Character("h".into()), &phys, tab);
    let palette = app.chrome.picker().expect("still browsing");
    assert_eq!(palette.dir.as_ref(), Some(&tree.0));
}

#[test]
fn test_browsing_onto_a_directory_opens_it_in_dir_editor_tab() {
    use winit::keyboard::NamedKey;

    let tree = TempTree::new("open-dir", &["src/", "README.md"]);
    let mut app = App::new();
    app.open_picker(crate::model::picker::Picker::open_files(tree.0.clone()));

    browse_to(&mut app, "src/");
    browse_key(&mut app, NamedKey::Enter);

    assert!(app.chrome.picker().is_none(), "the browser is done");
    let tab = app.layout().focused();
    assert_eq!(
        app.open_tabs.editor(&tab).map(|slot| slot.kind),
        Some(EditorKind::Dir),
        "and the directory is open in Dir editor"
    );
    let slot = app.open_tabs.editor(&tab).unwrap();
    assert_eq!(slot.editor.root_dir(), Some(tree.0.join("src")));
}

#[test]
fn test_each_file_opened_from_a_listing_gets_a_tab_of_its_own() {
    let tree = TempTree::new("two-files", &["one.txt", "two.txt"]);
    let mut app = App::new();
    let listing = app.show_editor(EditorKind::Grep, Box::new(CountingEditor::default()));

    app.open_text_editor(OpenTarget::file(tree.0.join("one.txt")));
    let one = app.layout().focused();
    app.show_tab(listing);
    app.open_text_editor(OpenTarget::file(tree.0.join("two.txt")));
    let two = app.layout().focused();

    assert_ne!(one, two, "two editor tabs");
    assert_eq!(
        app.open_tabs.editor(&one).map(|slot| slot.kind),
        Some(EditorKind::Text)
    );
    assert_eq!(
        app.open_tabs.editor(&two).map(|slot| slot.kind),
        Some(EditorKind::Text)
    );
}

#[test]
fn test_a_file_opened_from_a_text_tab_gets_a_tab_of_its_own() {
    // One editor, one buffer: a second file is a second buffer, not a
    // second file inside the first editor.
    let tree = TempTree::new("join", &["one.txt", "two.txt"]);
    let mut app = App::new();
    app.open_text_editor(OpenTarget::file(tree.0.join("one.txt")));
    let one = app.layout().focused();
    app.open_text_editor(OpenTarget::file(tree.0.join("two.txt")));
    assert_eq!(app.open_tabs.editors().count(), 2);
    assert_ne!(app.layout().focused(), one, "the second is in front");
}

#[test]
fn test_browsing_onto_a_file_opens_it_in_an_editor_tab() {
    use winit::keyboard::NamedKey;

    let tree = TempTree::new("open", &["README.md"]);
    let mut app = App::new();
    app.open_picker(crate::model::picker::Picker::open_files(tree.0.clone()));

    browse_to(&mut app, "README.md");
    browse_key(&mut app, NamedKey::Enter);

    assert!(app.chrome.picker().is_none(), "the browser is done");
    let tab = app.layout().focused();
    assert_eq!(
        app.open_tabs.editor(&tab).map(|slot| slot.kind),
        Some(EditorKind::Text),
        "and the file is open in a tab of the pane"
    );
}

#[test]
fn test_another_editor_opens_beside_the_first_instead_of_toggling() {
    let mut app = App::new();
    app.open_keys_editor();
    app.open_dir_editor();
    assert_eq!(
        app.open_tabs.editors().count(),
        2,
        "each page has a tab of its own"
    );
    assert_eq!(app.layout().members().len(), 3);
}

#[test]
fn test_a_listing_rooted_elsewhere_does_not_stand_in_for_a_new_one() {
    let mut app = App::new();
    let work = app.layout().focused();
    let start = app.focused_start_dir();
    app.open_dir_editor_at(PathBuf::from("/"));
    app.show_tab(work);

    app.open_dir_editor();
    let opened = app.layout().focused();
    app.show_tab(work);
    app.open_dir_editor();

    assert_eq!(
        app.open_tabs
            .editor(&opened)
            .and_then(|slot| slot.editor.root_dir()),
        Some(start),
        "the new listing is rooted where the work tab is looking"
    );
    assert_eq!(app.layout().focused(), opened, "asked again, it is reused");
    assert_eq!(app.open_tabs.editors().count(), 2);
}

#[test]
fn test_ctrl_shift_c_with_nothing_selected_reaches_the_editor() {
    // Copy is a global chord, checked ahead of every editor. With nothing
    // selected it copies nothing, so an editor binding the same chord (the
    // Git editor commits on it) has to be the one that gets it.
    let (mut app, pane) = app_with_open_editor();
    app.keymap = crate::model::input::Keymap::default();
    app.input.modifiers = winit::event::Modifiers::from(
        winit::keyboard::ModifiersState::CONTROL | winit::keyboard::ModifiersState::SHIFT,
    );
    app.handle_key(
        winit::event::ElementState::Pressed,
        winit::keyboard::Key::Character("C".into()),
        winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyC),
    );
    assert_eq!(
        app.open_tabs.editor(&pane).unwrap().editor.title(),
        "Counting 1"
    );
}

#[test]
fn test_an_editor_closing_itself_closes_its_tab() {
    let (mut app, pane) = app_with_open_editor();
    assert!(app.offer_key_to_editor(pane, &press(KeyCode::Char('q'))));
    assert_eq!(app.open_tabs.buried(), [pane], "the page is buried");
    assert_eq!(app.layout().members().len(), 1, "the terminal tab is not");
}

#[test]
fn test_an_editor_running_a_command_closes_first() {
    // The editor is done once it has named the command, so the command
    // runs against the tab shown in its place.
    let (mut app, pane) = app_with_open_editor();
    let tabs_before = app.layout().members().len();

    assert!(app.offer_key_to_editor(pane, &press(KeyCode::Char('r'))));
    assert_eq!(app.open_tabs.buried(), [pane], "the page closed");
    assert_eq!(
        app.layout().members().len(),
        tabs_before,
        "and the command opened a tab in its place"
    );
}

#[test]
fn test_a_saved_editor_tab_opens_again_where_it_was() {
    let dir = std::env::temp_dir().join(format!("winter-restore-pages-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let file = dir.join("notes.md");
    std::fs::write(&file, "one\ntwo\nthree\n").expect("write");
    let mut app = App::new();
    app.show_editor(EditorKind::Dir, Box::new(DirEditor::new(dir.clone())));
    app.open_path_editor(OpenTarget::at_line(file.clone(), 3));
    let saved = app.editor_sessions();
    assert_eq!(saved.len(), 2, "both pages say where they were: {saved:?}");

    let mut fresh = App::new();
    for editor in &saved {
        assert!(
            fresh.restore_editor(TabId(100 + editor.id as u64), editor),
            "{editor:?}"
        );
    }
    let editor = saved
        .iter()
        .find(|editor| editor.kind == EditorKind::Text.name())
        .expect("the editor");
    assert_eq!(editor.line, Some(3), "at the line it was on");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_resize_leaves_an_editor_tab_showing_its_editor() {
    // Resizing walks every laid-out pane and reflows the grids of the
    // terminals; an editor tab, which has none, must survive the pass.
    let (mut app, pane) = app_with_open_editor();
    app.resize_all_panes();
    assert!(app.open_tabs.is_editor(&pane));
}

#[test]
fn test_reveal_held_file_finds_tab_in_another_pane() {
    let dir = std::env::temp_dir().join(format!("winter-cross-pane-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let file = dir.join("main.tex");
    std::fs::write(&file, "Hello world\nLine 2\n").expect("file");

    let mut app = App::new();
    let editor = crate::editors::text::TextEditor::new(file.clone(), None).expect("opens");
    let tab1 = app.show_editor(EditorKind::Text, Box::new(editor));

    // Split to create a second pane with a second editor.
    let other_file = dir.join("other.txt");
    std::fs::write(&other_file, "other\n").expect("file");
    let other_editor = crate::editors::text::TextEditor::new(other_file, None).expect("opens");
    let tab2 = app.show_editor_split(EditorKind::Text, Box::new(other_editor));
    assert_eq!(app.layout().focused(), tab2);

    // Open target for main.tex while focused on tab2 in the second pane.
    app.open_path_editor(OpenTarget::at_line(file, 2));

    // Focus must switch to tab1 in the first pane without creating a new tab.
    assert_eq!(app.layout().focused(), tab1);
    assert_eq!(app.layout().shown_tabs().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}
