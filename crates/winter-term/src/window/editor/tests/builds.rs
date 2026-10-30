//! The window's editor tests: builds, tasks, and SyncTeX.

use super::*;

#[cfg(unix)]
#[test]
fn test_saving_a_tex_file_builds_its_document_only_when_asked_and_keeps_the_focus() {
    let dir = latex_project("save");
    let root = dir.join("main.tex");
    let mut app = App::new();
    app.config.compile.latex_command = "true".to_string();
    app.open_text_editor(OpenTarget::file(root.clone()));
    let editing = app.layout().focused();
    app.on_file_written(&root);
    assert!(compile_tabs(&app).is_empty(), "off unless the settings ask");

    app.config.compile.latex_on_save = true;
    app.on_file_written(&root);
    app.on_file_written(&root);
    assert_eq!(compile_tabs(&app).len(), 1, "one tab, rebuilt in place");
    assert_eq!(app.layout().focused(), editing, "the file keeps the focus");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_f8_from_a_file_walks_the_newest_build() {
    let dir = latex_project("walk");
    let mut app = App::new();
    app.open_text_editor(OpenTarget::file(dir.join("main.tex")));
    let editing = app.layout().focused();
    // Not from the .tex file, whose document would be built first: from
    // a listing of the project, which builds the task list's first.
    app.show_editor(EditorKind::Dir, Box::new(LocatedEditor(dir.clone())));
    app.recompile();
    tick_until_idle(&mut app);
    assert_eq!(compile_tabs(&app).len(), 1);

    app.set_focus(editing);
    app.step_error(editing, true);
    assert!(
        app.chrome.notice_text()
            .is_some_and(|notice| notice.starts_with("1 of 1: ")),
        "{:?}",
        app.chrome.notice_text()
    );
    assert_eq!(app.newest_build(), compile_tabs(&app).first().copied());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_the_task_list_comes_before_any_tab_and_a_choice_runs_beside_the_work() {
    let dir = task_project("pick", &[("first", "true"), ("second", "true")]);
    let mut app = App::new();
    app.show_editor(EditorKind::Dir, Box::new(LocatedEditor(dir.clone())));
    let working = app.layout().focused();
    app.compile_task();
    assert_eq!(
        app.chrome.picker().map(|p| p.mode.clone()),
        Some(PickerMode::Tasks)
    );
    assert!(compile_tabs(&app).is_empty(), "nothing is opened to ask");
    let labels: Vec<String> = app
        .chrome.picker()
        .map(|p| p.entries.iter().map(|e| e.label.clone()).collect())
        .unwrap_or_default();
    assert_eq!(labels, ["first", "second", "Type a command..."]);

    choose_palette_entry(&mut app, 1);
    assert_eq!(compile_tabs(&app).len(), 1);
    let compile_tab = compile_tabs(&app)[0];
    assert!(
        app.layout().in_dock(compile_tab),
        "the build shows in the dock by default"
    );
    assert_eq!(
        app.layout().focused(),
        working,
        "the focus stays on the work"
    );
    assert_eq!(
        app.layout().shown_tabs().len(),
        2,
        "the build shows in the dock"
    );
    tick_until_idle(&mut app);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_compile_runs_beside_when_dock_disabled() {
    let dir = task_project("dock-off", &[("first", "true"), ("second", "true")]);
    let mut app = App::new();
    app.config.compile.dock = false;
    app.show_editor(EditorKind::Dir, Box::new(LocatedEditor(dir.clone())));
    let working = app.layout().focused();
    app.compile_task();
    choose_palette_entry(&mut app, 0);

    let compile_tab = compile_tabs(&app)[0];
    assert!(
        !app.layout().in_dock(compile_tab),
        "the build does not show in dock when dock is disabled"
    );
    assert_eq!(app.layout().focused(), working);
    assert_eq!(app.layout().shown_tabs().len(), 2);
    tick_until_idle(&mut app);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_one_task_runs_at_once_in_a_tab_left_behind_the_work() {
    let dir = task_project("only", &[("only", "true")]);
    let mut app = App::new();
    app.show_editor(EditorKind::Dir, Box::new(LocatedEditor(dir.clone())));
    let working = app.layout().focused();
    app.compile_task();
    assert!(app.chrome.picker().is_none(), "nothing to choose, nothing asked");
    assert_eq!(compile_tabs(&app).len(), 1, "it runs");
    assert_eq!(app.layout().focused(), working, "the focus stays on the work");
    assert_eq!(app.layout().shown_tabs(), vec![working], "and nothing is brought to the front");
    tick_until_idle(&mut app);
    app.compile_task();
    assert_eq!(compile_tabs(&app).len(), 1, "again, in the same tab");
    tick_until_idle(&mut app);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_a_task_whose_compile_buffer_is_buried_runs_there_and_shows_it_again() {
    let dir = task_project("buried", &[("first", "true"), ("second", "true")]);
    let mut app = App::new();
    app.config.compile.dock = false;
    app.show_editor(EditorKind::Dir, Box::new(LocatedEditor(dir.clone())));
    let working = app.layout().focused();
    app.compile_task();
    choose_palette_entry(&mut app, 0);
    tick_until_idle(&mut app);
    let compile_tab = compile_tabs(&app)[0];
    app.close_editor(compile_tab);
    assert_eq!(app.open_tabs.buried(), [compile_tab]);

    app.compile_task();
    choose_palette_entry(&mut app, 0);
    assert_eq!(compile_tabs(&app), [compile_tab], "the same buffer, not a new one");
    assert!(app.layout().contains(compile_tab), "shown again");
    assert_eq!(app.layout().focused(), working, "without taking the focus");
    tick_until_idle(&mut app);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_each_task_runs_in_a_tab_of_its_own_and_again_in_the_same_one() {
    let dir = task_project("tabs", &[("a", "true"), ("b", "exit 0")]);
    let mut app = App::new();
    app.show_editor(EditorKind::Dir, Box::new(LocatedEditor(dir.clone())));
    for at in [0, 1, 0] {
        app.compile_task();
        choose_palette_entry(&mut app, at);
        tick_until_idle(&mut app);
    }
    assert_eq!(compile_tabs(&app).len(), 2, "one tab per task, reused");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_a_build_saves_the_files_being_edited_before_it_starts() {
    let dir = task_project("save-first", &[("show", "cat note.txt")]);
    let note = dir.join("note.txt");
    std::fs::write(&note, "old\n").expect("note");
    let mut app = App::new();
    app.open_text_editor(OpenTarget::file(note.clone()));
    let editing = app.layout().focused();
    for key in [press(KeyCode::Char('c')), press(KeyCode::Char('c'))] {
        app.offer_key_to_editor(editing, &key);
    }
    for c in "new".chars() {
        app.offer_key_to_editor(editing, &press(KeyCode::Char(c)));
    }
    app.offer_key_to_editor(editing, &press(KeyCode::Escape));
    assert!(app
        .open_tabs
        .editor(&editing)
        .is_some_and(|slot| slot.editor.is_dirty()));
    app.show_editor(EditorKind::Dir, Box::new(LocatedEditor(dir.clone())));
    app.recompile();
    assert_eq!(std::fs::read_to_string(&note).expect("reads"), "new\n");
    tick_until_idle(&mut app);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_a_place_opened_from_the_compile_tab_opens_beside_it() {
    let dir = task_project("beside", &[("err", "printf 'a.c:1:3: error: x\\\\n'")]);
    std::fs::write(dir.join("a.c"), "int x;\n").expect("source");
    let mut app = App::new();
    app.show_editor(EditorKind::Dir, Box::new(LocatedEditor(dir.clone())));
    app.recompile();
    tick_until_idle(&mut app);
    let compile = compile_tabs(&app)[0];
    app.set_focus(compile);
    app.open_beside(compile, OpenTarget::at(dir.join("a.c"), 1, Some(3)));
    let shown = app.layout().shown_tabs();
    assert!(
        shown.contains(&compile),
        "the build stays in sight: {shown:?}"
    );
    let opened = app.layout().focused();
    assert_ne!(opened, compile);
    assert_eq!(
        app.open_tabs.editor(&opened).map(|slot| slot.kind()),
        Some(EditorKind::Text)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_saving_runs_the_projects_on_save_tasks_once_they_are_trusted() {
    // A cloned repository's tasks file ran its commands on every save,
    // with nothing asking whether that file was to be trusted.
    let dir = task_project("on-save", &[]);
    let tasks = "task \"check\" \"true\" on-save=#true\ntask \"other\" \"true\"\n";
    std::fs::write(dir.join(".winter/tasks.kdl"), tasks).expect("task list");
    let file = dir.join("a.txt");
    std::fs::write(&file, "x\n").expect("file");
    let mut app = App::new();
    app.on_file_written(&file);
    assert!(compile_tabs(&app).is_empty(), "not trusted, so nothing ran");

    crate::config::trust_tasks(&crate::project::root(&dir), tasks);
    app.on_file_written(&file);
    assert_eq!(compile_tabs(&app).len(), 1, "only the on-save task");
    tick_until_idle(&mut app);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_synctex_shows_the_pdf_in_this_pane_or_beside_and_reuses_its_tab() {
    let dir = std::env::temp_dir().join(format!("winter-show-pdf-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let pdf = dir.join("main.pdf");
    std::fs::write(&pdf, "").expect("pdf");
    let pdf_tabs = |app: &App| app.open_tabs.editors().filter(|(_, slot)| slot.kind() == EditorKind::Pdf).count();

    let mut app = App::new();
    app.show_in_pdf(pdf.clone(), None, false);
    assert_eq!((app.layout().shown_tabs().len(), pdf_tabs(&app)), (1, 1), "a tab of this pane");
    app.show_in_pdf(pdf.clone(), None, false);
    assert_eq!(pdf_tabs(&app), 1, "the open tab is shown again");

    let mut app = App::new();
    app.show_in_pdf(pdf.clone(), None, true);
    assert_eq!((app.layout().shown_tabs().len(), pdf_tabs(&app)), (2, 1), "beside, in a pane of its own");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_pdf_editor_run_command_synctex_backward() {
    let dir = std::env::temp_dir().join(format!("winter-pdf-cmd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let pdf_path = dir.join("test.pdf");
    std::fs::write(&pdf_path, "").expect("pdf");

    let mut pdf = crate::editors::pdf::PdfEditor::new(pdf_path);
    assert!(pdf.run_command(crate::model::editor::SYNCTEX_BACKWARD).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}
