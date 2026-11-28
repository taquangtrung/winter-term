//! The process monitor's tests.

use misty::testing::{TestWindow, access_tree, drawn_text};
use misty::{ButtonInput, Point, PointerButton};
use winter_model::editor::CommandRequest;
use winter_model::input::KeyCode;
use winter_model::process::{CpuReading, RawProcess};

use super::act::Signal;
use super::*;

use winter_model::notice::SearchMiss;

/// The mark on a name too long for its column.
const ELLIPSIS: char = '\u{2026}';

fn press(code: KeyCode) -> Key {
    Key::plain(code)
}

fn raw(pid: u32, ppid: u32, name: &str, ms: u64) -> RawProcess {
    RawProcess {
        command: format!("{name} --serve"),
        cpu: CpuReading::Time(ms),
        cwd: Some(format!("/work/{name}")),
        exec_path: None,
        gpu_kb: None,
        name: name.to_string(),
        pid,
        ppid: Some(ppid),
        rss_kb: 1024,
        state: ProcState::Sleeping,
        user: Some("me".to_string()),
    }
}

fn sample_at(taken: Instant, ms: [u64; 3]) -> ProcessSample {
    ProcessSample {
        cores: 1,
        entries: vec![
            raw(1, 0, "init", ms[0]),
            raw(10, 1, "server", ms[1]),
            raw(11, 10, "worker", ms[2]),
        ],
        has_gpu: false,
        taken,
        total_mem_kb: 1_000_000,
    }
}

fn loaded_editor() -> ProcEditor {
    let mut editor = ProcEditor::new();
    editor.on_job(JobReply::Processes(Ok(sample_at(
        Instant::now(),
        [0, 0, 0],
    ))));
    editor
}

/// Switch `editor` from the flat list it starts in to the tree.
fn in_tree(editor: &mut ProcEditor) {
    editor.on_key(&press(KeyCode::Char('=')));
}

fn cursor_on(editor: &mut ProcEditor, name: &str) {
    let at = editor
        .rows
        .iter()
        .position(|row| editor.processes[row.index].name == name)
        .expect("process is listed");
    editor.move_to(at);
}

fn command_of(outcome: EditorOutcome) -> CommandRequest {
    let EditorOutcome::Job(JobRequest::Command(request)) = outcome else {
        panic!("expected a command, got {outcome:?}");
    };
    request
}

#[test]
fn test_the_first_tick_asks_for_a_snapshot_and_a_pending_one_is_not_asked_twice() {
    let mut editor = ProcEditor::new();
    let now = Instant::now();
    assert_eq!(
        editor.on_tick(now),
        EditorOutcome::Job(JobRequest::Processes)
    );
    // With one outstanding, the next wake is the give-up deadline, not
    // another request.
    let deadline = editor.next_tick().unwrap();
    assert!(deadline >= now + STALL_TIMEOUT - Duration::from_secs(1));
}

#[test]
fn test_a_paused_editor_asks_for_nothing_and_wakes_nobody() {
    let mut editor = loaded_editor();
    editor.on_key(&press(KeyCode::Char('p')));
    assert_eq!(editor.next_tick(), None);
    assert_eq!(editor.on_tick(Instant::now()), EditorOutcome::Consumed);
}

#[test]
fn test_the_cursor_stays_on_its_process_when_a_refresh_reorders_the_list() {
    let mut editor = ProcEditor::new();
    let start = Instant::now();
    editor.on_job(JobReply::Processes(Ok(sample_at(start, [0, 0, 0]))));
    cursor_on(&mut editor, "worker");
    // Now the worker is the busiest and sorts to the top of the flat list.
    let later = start + Duration::from_secs(2);
    editor.on_job(JobReply::Processes(Ok(sample_at(later, [0, 100, 2000]))));
    assert_eq!(editor.selected().unwrap().name, "worker");
    assert_eq!(editor.list.cursor(), 0);
}

#[test]
fn test_a_process_that_exits_leaves_the_cursor_on_the_same_row() {
    let mut editor = loaded_editor();
    editor.on_key(&press(KeyCode::Char('=')));
    editor.move_to(2);
    let mut smaller = sample_at(Instant::now(), [0, 0, 0]);
    smaller.entries.pop();
    editor.on_job(JobReply::Processes(Ok(smaller)));
    // Row two no longer exists, so the cursor lands on the last row there
    // is rather than on nothing.
    assert!(editor.list.cursor() < editor.rows.len());
    assert!(editor.selected().is_some());
}

#[test]
fn test_terminating_asks_first_and_names_the_process() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    let EditorOutcome::Confirm(request) = editor.on_key(&press(KeyCode::Char('x'))) else {
        panic!("expected a confirmation");
    };
    assert!(request.is_destructive(), "ending a process warns");
    assert!(
        request.message().contains("server"),
        "got {:?}",
        request.message()
    );
    assert!(
        request.message().contains("10"),
        "got {:?}",
        request.message()
    );
}

#[cfg(unix)]
#[test]
fn test_confirming_sends_the_signal_to_the_selected_pid_only() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    editor.on_key(&press(KeyCode::Char('x')));
    let request = command_of(editor.on_confirm(true));
    assert_eq!(request.args, ["-TERM", "10"]);
}

#[test]
fn test_a_pid_handed_to_another_program_while_asking_is_not_signalled() {
    // The yes was about `server`; the pid it had came back as something
    // else by the time the answer did, and that got the signal.
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    editor.on_key(&press(KeyCode::Char('x')));
    let mut reused = sample_at(Instant::now(), [0, 0, 0]);
    reused.entries[1] = raw(10, 1, "editor", 0);
    editor.on_job(JobReply::Processes(Ok(reused)));
    let outcome = editor.on_confirm(true);
    assert_eq!(outcome, EditorOutcome::Consumed, "nothing was signalled");
}

#[cfg(unix)]
#[test]
fn test_a_tree_kill_reaches_the_children_first() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    editor.on_key(&press(KeyCode::Char('T')));
    let request = command_of(editor.on_confirm(true));
    assert_eq!(request.args, ["-KILL", "11", "10"]);
}

#[test]
fn test_declining_the_confirmation_runs_nothing() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    editor.on_key(&press(KeyCode::Char('X')));
    let outcome = editor.on_confirm(false);
    assert_eq!(outcome, EditorOutcome::Consumed);
    // And the declined kill must not linger to be fired by a later,
    // unrelated answer.
    let stray = editor.on_confirm(true);
    assert_eq!(stray, EditorOutcome::Consumed);
}

#[cfg(unix)]
#[test]
fn test_suspending_needs_no_confirmation() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    let request = command_of(editor.on_key(&press(KeyCode::Char('z'))));
    assert_eq!(request.args, ["-STOP", "10"]);
}

#[test]
fn test_a_finished_signal_reports_what_it_did_and_asks_for_a_fresh_list() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    editor.on_key(&press(KeyCode::Char('z')));
    let before = Instant::now();
    editor.on_job(JobReply::Command(CommandOutput {
        id: 0,
        code: Some(0),
        stderr: String::new(),
        stdout: String::new(),
        tag: TAG_SIGNAL,
    }));
    assert_eq!(
        editor.notice.take(),
        Some(ProcNotice::Done(Acting::Signalled(Signalled {
            name: "server".to_string(),
            pid: 10,
            signal: Signal::Stop,
        })))
    );
    assert!(editor.next_tick().unwrap() <= Instant::now().max(before));
}

#[test]
fn test_a_failed_signal_says_why() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    editor.on_key(&press(KeyCode::Char('z')));
    editor.on_job(JobReply::Command(CommandOutput {
        id: 0,
        code: Some(1),
        stderr: "kill: (10): Operation not permitted\n".to_string(),
        stdout: String::new(),
        tag: TAG_SIGNAL,
    }));
    let Some(ProcNotice::Failed(detail)) = editor.notice.take() else {
        panic!("the failure is reported");
    };
    assert!(detail.contains("Operation not permitted"), "{detail}");
}

#[test]
fn test_a_filter_answer_narrows_the_list_and_escape_clears_it_and_never_closes() {
    let mut editor = loaded_editor();
    in_tree(&mut editor);
    editor.on_prompt(PromptReply {
        answer: Some(" work ".to_string()),
        tag: ASK_FILTER,
    });
    // Tree mode keeps the worker's ancestors above it.
    assert_eq!(editor.rows.len(), 3);
    editor.on_key(&press(KeyCode::Char('=')));
    assert_eq!(editor.rows.len(), 1);
    assert_eq!(
        editor.on_key(&press(KeyCode::Escape)),
        EditorOutcome::Consumed
    );
    assert_eq!(editor.rows.len(), 3);
    assert_ne!(editor.on_key(&press(KeyCode::Escape)), EditorOutcome::Close);
}

#[test]
fn test_f_asks_for_a_filter_and_slash_for_a_search() {
    let mut editor = loaded_editor();
    let asked = |outcome: EditorOutcome| match outcome {
        EditorOutcome::Prompt(request) => request.tag,
        other => panic!("expected a prompt, got {other:?}"),
    };
    assert_eq!(asked(editor.on_key(&press(KeyCode::Char('f')))), ASK_FILTER);
    assert_eq!(asked(editor.on_key(&press(KeyCode::Char('/')))), ASK_SEARCH);
}

#[test]
fn test_a_search_moves_to_the_match_keeps_the_list_and_n_wraps_back() {
    let mut editor = loaded_editor();
    let listed = editor.rows.len();
    cursor_on(&mut editor, "server");
    editor.on_prompt(PromptReply {
        answer: Some("work".to_string()),
        tag: ASK_SEARCH,
    });
    assert_eq!(editor.selected().unwrap().name, "worker");
    assert_eq!(editor.rows.len(), listed, "a search narrows nothing");
    editor.on_key(&press(KeyCode::Char('n')));
    assert_eq!(editor.selected().unwrap().name, "worker", "the only match");
    editor.on_prompt(PromptReply {
        answer: Some("absent".to_string()),
        tag: ASK_SEARCH,
    });
    assert_eq!(
        editor.selected().unwrap().name,
        "worker",
        "a miss stays put"
    );
    assert_eq!(
        editor.notice.take(),
        Some(ProcNotice::Search(SearchMiss::NotFound(
            "absent".to_string()
        ))),
        "and says so"
    );
}

#[test]
fn test_folding_hides_children_and_the_cursor_row_survives() {
    let mut editor = loaded_editor();
    in_tree(&mut editor);
    cursor_on(&mut editor, "server");
    editor.on_key(&press(KeyCode::Tab));
    assert_eq!(editor.rows.len(), 2);
    assert_eq!(editor.selected().unwrap().name, "server");
    editor.on_key(&press(KeyCode::Tab));
    assert_eq!(editor.rows.len(), 3);
}

#[test]
fn test_the_editor_reports_the_selected_process_directory() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "worker");
    assert_eq!(editor.cwd(), Some(PathBuf::from("/work/worker")));
}

/// A window `height` pixels tall.
fn window(height: u32) -> TestWindow {
    TestWindow::new(1400, height)
}

/// `editor` drawn over `window`, told the window's text rows and columns
/// first as the window tells a page.
fn drawn(editor: &mut ProcEditor, window: &TestWindow) -> misty::Frame {
    let mut frame = window.frame();
    let bounds = frame.bounds();
    let cell = frame.cell_metrics();
    editor.content(
        (bounds.height / cell.height) as usize,
        (bounds.width / cell.width) as usize,
        false,
    );
    editor.draw_view(&mut frame, bounds, &misty::Theme::dark());
    let nested = misty::testing::nested_scrolls(&frame);
    assert!(
        nested.is_empty(),
        "scrolls inside a scrolling page: {nested:?}"
    );
    frame
}

/// The text `editor` draws over `window`.
fn shown(editor: &mut ProcEditor, window: &TestWindow) -> String {
    drawn_text(&drawn(editor, window)).join("\n")
}

/// The middle of the heading saying `label`, its sort arrow aside.
fn heading_at(frame: &misty::Frame, label: &str) -> Point {
    let tree = access_tree(frame, "proc");
    let bounds = tree
        .nodes
        .iter()
        .find(|(_, node)| {
            node.label()
                .is_some_and(|name| name.trim_start().ends_with(label))
        })
        .and_then(|(_, node)| node.bounds())
        .unwrap_or_else(|| panic!("a {label:?} heading"));
    Point::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    )
}

/// A left click at `point`.
fn click_at(editor: &mut ProcEditor, window: &mut TestWindow, point: Point) {
    for input in [
        ButtonInput::press(PointerButton::Left, point),
        ButtonInput::release(PointerButton::Left, point),
    ] {
        editor.view_input(&Input::PointerButton(input), &mut window.cx());
    }
}

/// Click the heading saying `label`.
fn click(editor: &mut ProcEditor, label: &str) {
    let mut window = window(900);
    let point = heading_at(&drawn(editor, &window), label);
    click_at(editor, &mut window, point);
}

/// Click list row `at`, counted from the first row drawn: a text row and
/// the theme's row padding above and below each, under the headings and
/// the line under them.
fn click_row(editor: &mut ProcEditor, at: usize) {
    let mut window = window(900);
    let frame = drawn(editor, &window);
    let line = frame.cell_metrics().height;
    let heading = heading_at(&frame, "PID");
    let padding = f32::from(misty::Theme::dark().shape.row_padding);
    let row = line + 2.0 * padding;
    let below = line / 2.0 + padding + 1.0;
    let y = heading.y + below + (at as f32 + 0.5) * row;
    click_at(editor, &mut window, Point::new(heading.x, y));
}

#[test]
fn test_a_failed_snapshot_is_shown_and_retried() {
    let mut editor = ProcEditor::new();
    editor.on_tick(Instant::now());
    editor.on_job(JobReply::Processes(Err("ps: not found".to_string())));
    let text = shown(&mut editor, &window(600));
    assert!(text.contains("ps: not found"), "got {text:?}");
    assert!(editor.next_tick().is_some());
}

#[test]
fn test_a_click_on_the_header_s_layout_choice_lists_the_tree() {
    let mut editor = loaded_editor();
    let text = shown(&mut editor, &window(900));
    let (header, _) = text.split_once("\nPID\n").expect("the headings");
    assert!(header.starts_with("System\n›\nProcess Monitor"), "{text:?}");
    assert!(
        header.contains("Flat\nTree"),
        "the choices over the headings"
    );
    click(&mut editor, "Tree");
    assert_eq!(editor.mode, ViewMode::Tree);
    click(&mut editor, "Flat");
    assert_eq!(editor.mode, ViewMode::Flat);
}

#[test]
fn test_choosing_another_system_page_in_the_path_asks_the_window_for_it() {
    let mut editor = loaded_editor();
    click(&mut editor, "System");
    let mut window = window(900);
    let point = heading_at(&drawn(&mut editor, &window), "Port Monitor");
    let mut asked = None;
    for input in [
        ButtonInput::press(PointerButton::Left, point),
        ButtonInput::release(PointerButton::Left, point),
    ] {
        asked = asked.or(editor.view_input(&Input::PointerButton(input), &mut window.cx()));
    }
    assert_eq!(
        asked,
        Some(EditorOutcome::RunActionHere("ports_monitor".to_string()))
    );
}

fn gpu_editor() -> ProcEditor {
    let mut editor = ProcEditor::new();
    let mut sample = sample_at(Instant::now(), [0, 0, 0]);
    sample.has_gpu = true;
    sample.entries[1].gpu_kb = Some(2048);
    editor.on_job(JobReply::Processes(Ok(sample)));
    editor
}

#[test]
fn test_clicking_a_heading_sorts_by_that_column_in_its_starting_direction() {
    let mut editor = loaded_editor();
    click(&mut editor, "PID");
    assert_eq!(
        (editor.sort, editor.direction),
        (SortKey::Pid, SortDirection::Ascending)
    );
    click(&mut editor, "NAME");
    assert_eq!(
        (editor.sort, editor.direction),
        (SortKey::Name, SortDirection::Ascending)
    );
    click(&mut editor, "MEM");
    assert_eq!(
        (editor.sort, editor.direction),
        (SortKey::Memory, SortDirection::Descending)
    );
    click(&mut editor, "STATE");
    assert_eq!(editor.sort, SortKey::State);
    click(&mut editor, "USER");
    assert_eq!(editor.sort, SortKey::User);
    click(&mut editor, COMMAND_LABEL);
    assert_eq!(editor.sort, SortKey::Command);
}

#[test]
fn test_clicking_the_sorted_heading_again_reverses_it() {
    let mut editor = loaded_editor();
    click(&mut editor, "PID");
    click(&mut editor, "PID");
    assert_eq!(
        (editor.sort, editor.direction),
        (SortKey::Pid, SortDirection::Descending)
    );
    click(&mut editor, "PID");
    assert_eq!(editor.direction, SortDirection::Ascending);
}

#[test]
fn test_the_video_memory_column_appears_only_when_a_gpu_editor_answered() {
    // The footer always has a VRAM cell; the table's heading is what may not.
    let heading = |text: &str| text.split("COMMAND").next().unwrap_or_default().to_string();
    let plain = shown(&mut loaded_editor(), &window(900));
    assert!(!heading(&plain).contains("VRAM"));
    let mut editor = gpu_editor();
    let text = shown(&mut editor, &window(900));
    assert!(heading(&text).contains("VRAM"));
    // The one with a reading shows it, and the others show a dash rather
    // than a zero they were never measured at.
    assert!(text.contains("2.0M"), "got {text:?}");
    assert!(
        text.lines().any(|line| line.trim() == NO_VRAM),
        "got {text:?}"
    );
    click(&mut editor, "VRAM");
    assert_eq!(editor.sort, SortKey::Gpu);
}

#[test]
fn test_stepping_the_sort_skips_a_column_that_is_not_shown() {
    let mut editor = loaded_editor();
    editor.sort_by(SortKey::Memory);
    editor.on_key(&press(KeyCode::Char('s')));
    assert_eq!(editor.sort, SortKey::State);
    let mut with_gpu = gpu_editor();
    with_gpu.sort_by(SortKey::Memory);
    with_gpu.on_key(&press(KeyCode::Char('s')));
    assert_eq!(with_gpu.sort, SortKey::Gpu);
}

#[test]
fn test_clicking_a_process_moves_the_cursor_onto_it() {
    let mut editor = loaded_editor();
    in_tree(&mut editor);
    // Rows are init, server, worker, top down.
    click_row(&mut editor, 2);
    assert_eq!(editor.selected().unwrap().name, "worker");
    click_row(&mut editor, 0);
    assert_eq!(editor.selected().unwrap().name, "init");
}

#[test]
fn test_a_click_below_the_last_process_selects_nothing() {
    let mut editor = loaded_editor();
    click_row(&mut editor, 1);
    click_row(&mut editor, 3);
    assert_eq!(editor.selected().unwrap().name, "server");
}

#[test]
fn test_the_headings_sit_at_the_top_of_the_page() {
    // The tab names the monitor, so only the header and its line come
    // first: no title band and no padding.
    let mut editor = loaded_editor();
    let frame = drawn(&mut editor, &window(900));
    let row = frame.cell_metrics().height;
    let heading = heading_at(&frame, "PID");
    let cx = misty::MeasureCx::new(&frame, &misty::Theme::dark());
    let header = cx.bar_height(misty::ControlSize::Compact);
    assert!(
        heading.y > header && heading.y < header + row + 2.0,
        "the headings' middle at {}, a row {row} tall, under a header {header} tall",
        heading.y
    );
}

#[test]
fn test_the_footer_describes_the_selected_process() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "worker");
    let text = shown(&mut editor, &window(900));
    for words in [
        "\nPPID\n10 (server)\n",
        "\nCWD\n/work/worker",
        "\nCHILDREN\n0",
    ] {
        assert!(text.contains(words), "{words:?} in {text:?}");
    }
}

#[test]
fn test_the_footer_keeps_its_cells_with_nothing_selected() {
    let mut editor = loaded_editor();
    let selected = shown(&mut editor, &window(900));
    editor.rows.clear();
    let blank = shown(&mut editor, &window(900));
    assert!(blank.contains("no process selected"), "got {blank:?}");
    let cells = |text: &str| text.split("\nPID\n").last().unwrap().lines().count();
    assert_eq!(cells(&blank), cells(&selected));
}

#[test]
fn test_a_short_pane_drops_the_footer_to_keep_the_list() {
    let mut editor = loaded_editor();
    let row = window(900).frame().cell_metrics().height;
    let short = ((MIN_ROWS_FOR_DETAIL - 1) as f32 * row) as u32;
    let text = shown(&mut editor, &window(short));
    assert!(!text.contains("\nCWD\n"), "got {text:?}");
    assert!(text.contains("worker"), "the list kept");
}

/// What the NAME cell of the process `name` holds: the tree's indent and
/// marker, then the name.
fn name_cell_text(editor: &ProcEditor, name: &str) -> String {
    let at = editor
        .rows
        .iter()
        .position(|row| editor.processes[row.index].name == name)
        .expect("process is listed");
    let (prefix, name) =
        editor.name_parts(&editor.processes[editor.rows[at].index], &editor.rows[at]);
    format!("{prefix}{name}")
}

#[test]
fn test_the_name_is_the_second_column_with_the_tree_in_it() {
    let mut editor = loaded_editor();
    in_tree(&mut editor);
    let text = shown(&mut editor, &window(900));
    let pid = text.find("PID").unwrap();
    let name = text.find("NAME").unwrap();
    let cpu = text.find("CPU%").unwrap();
    assert!(pid < name && name < cpu, "got {text:?}");
    // The command column carries the command alone; the fold marker and
    // indent belong to the name.
    assert!(name_cell_text(&editor, "server").contains("\u{25be} server"));
    assert!(text.contains("server --serve"), "got {text:?}");
    assert!(name_cell_text(&editor, "worker").starts_with("    "));
}

fn long_named_editor(name: &str) -> ProcEditor {
    let mut editor = ProcEditor::new();
    let mut sample = sample_at(Instant::now(), [0, 0, 0]);
    sample.entries[2].name = name.to_string();
    editor.on_job(JobReply::Processes(Ok(sample)));
    in_tree(&mut editor);
    editor
}

#[test]
fn test_the_name_column_grows_to_print_a_long_name_whole() {
    let name = "kworker/u56:0-flush-259:5";
    let editor = long_named_editor(name);
    // The worker sits two levels down: indent and marker, then the name.
    assert!(editor.name_width >= 2 * TREE_INDENT.len() + 2 + name.chars().count());
    assert!(name_cell_text(&editor, name).contains(name));
    assert!(!name_cell_text(&editor, name).contains(ELLIPSIS));
}

#[test]
fn test_the_name_column_stops_growing_at_its_cap_and_never_shrinks() {
    let mut editor = long_named_editor(&"x".repeat(100));
    assert_eq!(editor.name_width, MAX_NAME_WIDTH);
    let cell = name_cell_text(&editor, &"x".repeat(100));
    assert!(cell.contains(ELLIPSIS), "got {cell:?}");
    // A later list of short names does not pull the columns back in.
    editor.on_job(JobReply::Processes(Ok(sample_at(
        Instant::now(),
        [0, 0, 0],
    ))));
    assert_eq!(editor.name_width, MAX_NAME_WIDTH);
}

#[test]
fn test_the_list_starts_flat_with_the_bare_name() {
    let editor = loaded_editor();
    assert_eq!(editor.mode, ViewMode::Flat);
    assert!(name_cell_text(&editor, "worker").starts_with("worker"));
}

#[test]
fn test_a_name_too_long_for_its_column_is_cut_with_a_mark() {
    assert_eq!(fit("short", 10), "short");
    assert_eq!(fit("abcdefghij", 5), format!("abcd{ELLIPSIS}"));
    assert_eq!(fit("abc", 0), "");
}

#[test]
fn test_a_deep_process_keeps_its_name_visible() {
    let editor = loaded_editor();
    let deep = ViewRow {
        depth: 40,
        folded: false,
        has_children: false,
        index: 0,
    };
    let (prefix, name) = editor.name_parts(&editor.processes[0], &deep);
    let cell = format!("{prefix}{name}");
    // Forty levels of indent must not push "init" out of the column.
    assert!(cell.chars().count() <= editor.name_width, "got {cell:?}");
    assert!(cell.contains("init"), "got {cell:?}");
}

#[test]
fn test_sorting_by_name_orders_alphabetically() {
    let mut editor = loaded_editor();
    editor.on_key(&press(KeyCode::Char('=')));
    editor.sort_by(SortKey::Name);
    let names: Vec<&str> = editor
        .rows
        .iter()
        .map(|row| editor.processes[row.index].name.as_str())
        .collect();
    assert_eq!(names, ["init", "server", "worker"]);
}

#[test]
fn test_only_the_processes_around_the_cursor_are_built() {
    let mut editor = ProcEditor::new();
    let mut sample = sample_at(Instant::now(), [0, 0, 0]);
    sample.entries = (0..300)
        .map(|at| raw(100 + at, 1, &format!("task{at:03}"), 0))
        .collect();
    editor.on_job(JobReply::Processes(Ok(sample)));
    editor.on_key(&press(KeyCode::Char('=')));
    editor.sort_by(SortKey::Name);
    cursor_on(&mut editor, "task000");
    let window = window(600);
    let text = shown(&mut editor, &window);
    assert!(text.contains("task000"));
    assert!(!text.contains("task299"), "out of sight, not built");
    editor.on_key(&Key {
        shift: true,
        ..press(KeyCode::Char('G'))
    });
    let text = shown(&mut editor, &window);
    assert!(text.contains("task299"), "the cursor's row built");
}

#[test]
fn test_every_key_of_the_monitor_is_spelled_right() {
    assert_eq!(table::invalid_specs(&KEYS), Vec::<&str>::new());
}

#[test]
fn test_every_control_says_what_it_is() {
    let mut editor = loaded_editor();
    let frame = drawn(&mut editor, &TestWindow::new(1400, 900));
    let unnamed = misty::testing::unnamed_controls(&frame);
    assert!(unnamed.is_empty(), "{unnamed:?}");
}

#[test]
fn test_a_narrow_pane_leaves_out_the_least_needed_columns_first() {
    let mut editor = loaded_editor();
    let labels = |editor: &ProcEditor| -> Vec<&str> {
        editor.columns().iter().map(|column| column.label).collect()
    };
    editor.content(40, 200, false);
    assert!(
        labels(&editor).contains(&USER_LABEL),
        "all fit: {:?}",
        labels(&editor)
    );
    editor.content(40, 75, false);
    let narrow = labels(&editor);
    assert!(
        !narrow.contains(&USER_LABEL),
        "the user goes first: {narrow:?}"
    );
    assert!(
        narrow.contains(&CPU_LABEL) && narrow.contains(&NAME_LABEL),
        "{narrow:?}"
    );
    editor.content(40, 10, false);
    assert_eq!(
        labels(&editor),
        [PID_LABEL, NAME_LABEL, CPU_LABEL],
        "the three that stay"
    );
}

#[test]
fn test_a_command_repeating_the_name_is_left_blank() {
    let mut sample = sample_at(Instant::now(), [0, 0, 0]);
    sample.entries[0].command = "init".to_string();
    let mut editor = ProcEditor::new();
    editor.on_job(JobReply::Processes(Ok(sample)));
    in_tree(&mut editor);
    let shown = drawn_text(&drawn(&mut editor, &TestWindow::new(1600, 900))).join("\n");
    assert!(
        shown.contains("server --serve"),
        "a command with more says it: {shown:?}"
    );
    // init's row ends at its user, with no command cell repeating its name.
    assert!(
        shown.contains("sleep\nme\n\n10"),
        "nothing after init's user: {shown:?}"
    );
}

fn details_of(pid: u32) -> winter_model::monitor::ProcessDetails {
    use winter_model::monitor::{FileKind, OpenFile, ProcessDetails};
    ProcessDetails {
        files: Ok(vec![OpenFile {
            fd: Some(3),
            kind: FileKind::File,
            target: "/var/log/server.log".to_string(),
        }]),
        ..ProcessDetails::unread(pid, "not allowed: the process belongs to another user")
    }
}

#[test]
fn test_enter_looks_into_the_process_and_the_list_stops_reading_meanwhile() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    assert_eq!(
        editor.on_key(&press(KeyCode::Enter)),
        EditorOutcome::Job(JobRequest::Monitor(MonitorQuery::Details(10)))
    );
    assert_eq!(editor.next_tick(), None, "the list is out of sight");
    editor.on_job(JobReply::Monitor(MonitorAnswer::Details(details_of(10))));
    let text = drawn_text(&drawn(&mut editor, &window(900))).join("\n");
    assert!(text.contains("server (pid 10)"), "{text:?}");
    assert!(
        text.contains("Open Files (1)") && text.contains("/var/log/server.log"),
        "{text:?}"
    );
    assert!(
        text.contains("belongs to another user"),
        "a part not read says why: {text:?}"
    );

    assert_eq!(
        editor.on_key(&press(KeyCode::Char('r'))),
        EditorOutcome::Job(JobRequest::Monitor(MonitorQuery::Details(10)))
    );
    editor.on_key(&press(KeyCode::Char('q')));
    assert!(editor.details.is_none());
    assert!(
        editor.next_tick().is_some(),
        "back on the list, it reads again"
    );
}

#[test]
fn test_an_answer_for_another_process_is_not_shown_and_tab_still_folds() {
    let mut editor = loaded_editor();
    in_tree(&mut editor);
    cursor_on(&mut editor, "server");
    editor.on_key(&press(KeyCode::Enter));
    editor.on_job(JobReply::Monitor(MonitorAnswer::Details(details_of(99))));
    assert!(editor.details.as_ref().unwrap().state().info.is_none());
    editor.on_key(&press(KeyCode::Escape));
    editor.on_key(&press(KeyCode::Tab));
    assert!(
        editor.collapsed.contains(&10),
        "Tab folds, Enter no longer does"
    );
}

#[test]
fn test_every_control_of_the_details_says_what_it_is() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    editor.on_key(&press(KeyCode::Enter));
    editor.on_job(JobReply::Monitor(MonitorAnswer::Details(details_of(10))));
    let frame = drawn(&mut editor, &TestWindow::new(1400, 900));
    let unnamed = misty::testing::unnamed_controls(&frame);
    assert!(unnamed.is_empty(), "{unnamed:?}");
    assert_eq!(table::invalid_specs(&DETAIL_KEYS), Vec::<&str>::new());
}
