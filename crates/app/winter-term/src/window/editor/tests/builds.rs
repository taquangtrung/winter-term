//! The window's editor tests: builds, tasks, and SyncTeX.

use winter_model::layout::{BottomPanelKind, Edge, Panel, PanelState, SidebarKind};

use super::*;
use crate::window::panels::views::outline::OutlineView;
use crate::window::panels::views::{PanelPage, PanelView};

/// How many finished runs Output keeps, as the window caps them.
const MAX_KEPT: usize = 20;

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
        app.chrome
            .notice_text()
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
    assert!(app.is_picker_open("tasks"));
    assert!(compile_tabs(&app).is_empty(), "nothing is opened to ask");
    app.settle_picker();
    let labels = app.picker_labels();
    assert_eq!(labels, ["first", "second", "Type a command…"]);

    choose_palette_entry(&mut app, 1);
    assert_eq!(compile_tabs(&app).len(), 1);
    let compile_tab = compile_tabs(&app)[0];
    assert!(
        app.open_tabs.buried().contains(&compile_tab),
        "the build opens no tab"
    );
    assert_eq!(
        app.layout().bottom_panel().active_kind(),
        Some(BottomPanelKind::Output),
        "Output shows it"
    );
    assert_eq!(app.layout().panel_state(Panel::Bottom), PanelState::Shown);
    assert_eq!(
        app.layout().focused(),
        working,
        "the focus stays on the work"
    );
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
    assert!(
        app.chrome.picker().is_none(),
        "nothing to choose, nothing asked"
    );
    assert_eq!(compile_tabs(&app).len(), 1, "it runs");
    assert_eq!(
        app.layout().focused(),
        working,
        "the focus stays on the work"
    );
    assert_eq!(
        app.layout().shown_tabs(),
        vec![working],
        "and nothing is brought to the front"
    );
    tick_until_idle(&mut app);
    app.compile_task();
    assert_eq!(compile_tabs(&app).len(), 1, "again, in the same tab");
    tick_until_idle(&mut app);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_a_task_runs_again_in_its_buffer_and_output_shows_it_in_the_main_area_when_asked() {
    let dir = task_project("buried", &[("first", "true"), ("second", "true")]);
    let mut app = App::new();
    app.show_editor(EditorKind::Dir, Box::new(LocatedEditor(dir.clone())));
    let working = app.layout().focused();
    app.compile_task();
    choose_palette_entry(&mut app, 0);
    tick_until_idle(&mut app);
    let compile_tab = compile_tabs(&app)[0];

    app.compile_task();
    choose_palette_entry(&mut app, 0);
    assert_eq!(
        compile_tabs(&app),
        [compile_tab],
        "the same buffer, not a new one"
    );
    assert!(
        !app.layout().contains(compile_tab),
        "still out of the panes"
    );
    assert_eq!(app.layout().focused(), working, "without taking the focus");
    tick_until_idle(&mut app);

    // Output's `t` shows the build it is on in the main area.
    let output = app.open_bottom_view(BottomPanelKind::Output, true);
    // A frame finds it in sight, and feeds it the builds.
    app.sync_shown();
    app.sync_panel_views();
    if let Some(slot) = app.open_tabs.page_mut(&output) {
        slot.page.on_key(&Key::plain(KeyCode::Char('t')));
    }
    app.sync_shown();
    app.sync_panel_views();
    assert_eq!(app.layout().region_of(compile_tab), Some(Region::Main));
    assert_eq!(app.layout().focused(), compile_tab);
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
    assert!(app.open_tabs.page(&editing).is_some_and(|slot| {
        slot.page
            .as_editor()
            .is_some_and(|editor| editor.is_dirty())
    }));
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
    app.show_build_in_main_area(compile);
    app.open_beside(compile, OpenTarget::at(dir.join("a.c"), 1, Some(3)));
    let shown = app.layout().shown_tabs();
    assert!(
        shown.contains(&compile),
        "the build stays in sight: {shown:?}"
    );
    let opened = app.layout().focused();
    assert_ne!(opened, compile);
    assert_eq!(
        app.open_tabs.page(&opened).map(|slot| slot.kind()),
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

    crate::config::trust_tasks(&winter_base::project::root(&dir), tasks);
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
    let pdf_tabs = |app: &App| {
        app.open_tabs
            .pages()
            .filter(|(_, slot)| slot.kind() == EditorKind::Pdf)
            .count()
    };

    let mut app = App::new();
    app.show_in_pdf(pdf.clone(), None, false);
    assert_eq!(
        (app.layout().shown_tabs().len(), pdf_tabs(&app)),
        (1, 1),
        "a tab of this pane"
    );
    app.show_in_pdf(pdf.clone(), None, false);
    assert_eq!(pdf_tabs(&app), 1, "the open tab is shown again");

    let mut app = App::new();
    app.show_in_pdf(pdf.clone(), None, true);
    assert_eq!(
        (app.layout().shown_tabs().len(), pdf_tabs(&app)),
        (2, 1),
        "beside, in a pane of its own"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_pdf_editor_run_command_synctex_backward() {
    let dir = std::env::temp_dir().join(format!("winter-pdf-cmd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let pdf_path = dir.join("test.pdf");
    std::fs::write(&pdf_path, "").expect("pdf");

    let mut pdf = winter_pdf::PdfEditor::new(pdf_path);
    assert!(
        pdf.run_command(winter_model::editor::SYNCTEX_BACKWARD)
            .is_some()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_nothing_moves_into_a_panel() {
    let mut app = App::new();
    let main = app.layout().focused();
    let diff = app.show_editor(
        EditorKind::Diff,
        Box::new(LocatedEditor(PathBuf::from("/"))),
    );
    app.move_tab_to_pane(diff, main);
    app.toggle_panel(Panel::Sidebar(Edge::Left));
    let files = app.layout().focused();
    app.move_tab_to_pane(diff, files);
    app.move_tab_to(diff, files);
    assert_eq!(
        app.layout().region_of(diff),
        Some(Region::Main),
        "dropped there, refused"
    );
}

#[test]
fn test_the_new_tab_key_in_a_panel_opens_a_terminal_in_the_main_area() {
    let mut app = App::new();
    app.toggle_panel(Panel::Sidebar(Edge::Right));
    let before = app.layout().members().len();
    app.new_tab();
    let opened = app.layout().focused();
    assert_eq!(app.layout().members().len(), before + 1);
    assert_eq!(app.layout().region_of(opened), Some(Region::Main));
}

/// An editor with an outline of two entries, at its indexes 0 and 4, that
/// keeps the last jump made in it.
struct Outlined(std::rc::Rc<std::cell::Cell<Option<usize>>>);

impl Page for Outlined {
    fn title(&self) -> String {
        "notes.md".to_string()
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![vec![Span::plain("notes")]])
    }

    fn on_key(&mut self, _key: &Key) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    fn outline_targets(&mut self) -> Option<Vec<winter_model::editor::JumpTarget>> {
        let entry =
            |label: &str, target| winter_model::editor::JumpTarget::new(target, label.to_string());
        Some(vec![entry("One", 0), entry("Two", 4)])
    }

    fn on_jump(&mut self, step: winter_model::editor::JumpStep) {
        if let winter_model::editor::JumpStep::Confirm(target) = step {
            self.0.set(Some(target));
        }
    }
}

impl Editor for Outlined {}

#[test]
fn test_the_outline_follows_the_file_worked_on_and_takes_it_to_an_entry() {
    let jumped = std::rc::Rc::new(std::cell::Cell::new(None));
    let mut app = App::new();
    let text = app.show_editor(EditorKind::Text, Box::new(Outlined(jumped.clone())));
    app.open_sidebar_view(SidebarKind::Outline);
    let outline = app.layout().focused();
    assert_eq!(
        app.layout().region_of(outline),
        Some(Region::Panel(Panel::Sidebar(Edge::Right)))
    );
    app.sync_shown();
    app.sync_panel_views();
    let labels = app.open_tabs.page_mut(&outline).and_then(|slot| {
        slot.page
            .with_kind_mut(|page: &mut PanelPage<OutlineView>| {
                let rows = page.state_mut().list().rows();
                rows.iter().map(|row| row.label.clone()).collect::<Vec<_>>()
            })
    });
    assert_eq!(labels, Some(vec!["One".to_string(), "Two".to_string()]));
    if let Some(slot) = app.open_tabs.page_mut(&outline) {
        slot.page.on_key(&Key::plain(KeyCode::Char('G')));
        slot.page.on_key(&Key::plain(KeyCode::Enter));
    }
    app.sync_shown();
    app.sync_panel_views();
    assert_eq!(app.layout().focused(), text, "back to the file");
    assert_eq!(jumped.get(), Some(4), "at the second entry");
}

#[cfg(unix)]
#[test]
fn test_killing_a_run_s_buffer_leaves_it_in_output_and_d_lets_it_go() {
    let dir = task_project("keep", &[("check", "true")]);
    let mut app = App::new();
    app.show_editor(EditorKind::Dir, Box::new(LocatedEditor(dir.clone())));
    app.run_shell_task("true");
    tick_until_idle(&mut app);
    let [run] = compile_tabs(&app)[..] else {
        panic!("one run: {:?}", compile_tabs(&app));
    };
    app.show_build_in_main_area(run);
    app.kill_buffer(run);
    assert!(!app.layout().contains(run), "its tab closed");
    assert_eq!(app.builds_newest_first(), [run], "still in Output");

    app.remove_run(run);
    assert!(compile_tabs(&app).is_empty(), "d lets it go");
    assert!(app.builds_newest_first().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_a_run_still_going_is_not_removed() {
    let dir = task_project("going", &[("check", "true")]);
    let mut app = App::new();
    app.show_editor(EditorKind::Dir, Box::new(LocatedEditor(dir.clone())));
    app.run_shell_task("sleep 30");
    let [run] = compile_tabs(&app)[..] else {
        panic!("one run");
    };
    app.remove_run(run);
    assert_eq!(compile_tabs(&app), [run], "refused while it runs");
    app.kill_buffer_now(run);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn test_output_keeps_only_the_newest_finished_runs() {
    let dir = task_project("cap", &[("check", "true")]);
    let mut app = App::new();
    app.show_editor(EditorKind::Dir, Box::new(LocatedEditor(dir.clone())));
    for n in 0..MAX_KEPT + 2 {
        app.run_shell_task(&format!("true {n}"));
        tick_until_idle(&mut app);
    }
    assert_eq!(
        compile_tabs(&app).len(),
        MAX_KEPT + 1,
        "the cap, and the newest"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
