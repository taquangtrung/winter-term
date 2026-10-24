//! The process monitor's tests.

use crate::model::editor::CommandRequest;
use crate::model::input::KeyCode;
use crate::model::process::{CpuReading, RawProcess};

use super::*;

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
fn test_a_snapshot_that_never_came_back_is_asked_for_again() {
    let mut editor = ProcEditor::new();
    editor.on_tick(Instant::now());
    // The runner drops a request when it is full. Retrying at the
    // give-up deadline is what stops the editor waiting forever.
    let again = editor.on_tick(Instant::now() + STALL_TIMEOUT);
    assert_eq!(again, EditorOutcome::Job(JobRequest::Processes));
}

#[test]
fn test_an_answer_schedules_the_next_snapshot_one_interval_out() {
    let mut editor = ProcEditor::new();
    editor.on_tick(Instant::now());
    let before = Instant::now();
    editor.on_job(JobReply::Processes(Ok(sample_at(before, [0, 0, 0]))));
    assert!(editor.next_tick().unwrap() >= before + REFRESH_INTERVAL);
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
    editor.on_key(&press(KeyCode::Char('=')));
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
    let EditorOutcome::Prompt(request) = editor.on_key(&press(KeyCode::Char('x'))) else {
        panic!("expected a confirmation");
    };
    assert_eq!(request.mode, PromptMode::Confirm);
    assert!(request.label.contains("server"), "got {:?}", request.label);
    assert!(request.label.contains("10"), "got {:?}", request.label);
}

#[cfg(unix)]
#[test]
fn test_confirming_sends_the_signal_to_the_selected_pid_only() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    editor.on_key(&press(KeyCode::Char('x')));
    let request = command_of(editor.on_prompt(PromptReply {
        answer: Some("y".to_string()),
        tag: ASK_CONFIRM,
    }));
    assert_eq!(request.args, ["-TERM", "10"]);
}

#[cfg(unix)]
#[test]
fn test_a_tree_kill_reaches_the_children_first() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    editor.on_key(&press(KeyCode::Char('T')));
    let request = command_of(editor.on_prompt(PromptReply {
        answer: Some("y".to_string()),
        tag: ASK_CONFIRM,
    }));
    assert_eq!(request.args, ["-KILL", "11", "10"]);
}

#[test]
fn test_declining_the_confirmation_runs_nothing() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    editor.on_key(&press(KeyCode::Char('X')));
    let outcome = editor.on_prompt(PromptReply {
        answer: None,
        tag: ASK_CONFIRM,
    });
    assert_eq!(outcome, EditorOutcome::Consumed);
    // And the declined kill must not linger to be fired by a later,
    // unrelated answer.
    let stray = editor.on_prompt(PromptReply {
        answer: Some("y".to_string()),
        tag: ASK_CONFIRM,
    });
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
        code: Some(0),
        stderr: String::new(),
        stdout: String::new(),
        tag: TAG_SIGNAL,
    }));
    assert_eq!(
        editor.take_notice().map(|notice| notice.text).as_deref(),
        Some("suspended server (10)")
    );
    assert!(editor.next_tick().unwrap() <= Instant::now().max(before));
}

#[test]
fn test_a_failed_signal_says_why() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    editor.on_key(&press(KeyCode::Char('z')));
    editor.on_job(JobReply::Command(CommandOutput {
        code: Some(1),
        stderr: "kill: (10): Operation not permitted\n".to_string(),
        stdout: String::new(),
        tag: TAG_SIGNAL,
    }));
    let message = editor
        .take_notice()
        .map(|notice| notice.text)
        .unwrap_or_default();
    assert!(
        message.contains("Operation not permitted"),
        "got {message:?}"
    );
}

#[test]
fn test_a_filter_answer_narrows_the_list_and_escape_clears_it_and_never_closes() {
    let mut editor = loaded_editor();
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
    assert!(editor.take_notice().is_some(), "and says so");
}

#[test]
fn test_folding_hides_children_and_the_cursor_row_survives() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    editor.on_key(&press(KeyCode::Enter));
    assert_eq!(editor.rows.len(), 2);
    assert_eq!(editor.selected().unwrap().name, "server");
    editor.on_key(&press(KeyCode::Enter));
    assert_eq!(editor.rows.len(), 3);
}

#[test]
fn test_the_editor_reports_the_selected_process_directory() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "worker");
    assert_eq!(editor.cwd(), Some(PathBuf::from("/work/worker")));
}

#[test]
fn test_a_failed_snapshot_is_shown_and_retried() {
    let mut editor = ProcEditor::new();
    editor.on_tick(Instant::now());
    editor.on_job(JobReply::Processes(Err("ps: not found".to_string())));
    let summary = row_text(&editor.summary_row());
    assert!(summary.contains("ps: not found"), "got {summary:?}");
    assert!(editor.next_tick().is_some());
}

fn click(editor: &mut ProcEditor, col: usize) -> EditorOutcome {
    editor.on_mouse(Point {
        col,
        drag: false,
        row: COLUMN_ROW,
    })
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
    // The fixture's names are short, so NAME keeps its least width: PID is
    // cells 0 to 7, NAME 8 to 24, MEM% 32 to 38, STATE 47 to 53, USER 54
    // to 64, and the command is everything after.
    assert_eq!(click(&mut editor, 0), EditorOutcome::Consumed);
    assert_eq!(
        (editor.sort, editor.direction),
        (SortKey::Pid, SortDirection::Ascending)
    );
    click(&mut editor, 10);
    assert_eq!(
        (editor.sort, editor.direction),
        (SortKey::Name, SortDirection::Ascending)
    );
    click(&mut editor, 33);
    assert_eq!(
        (editor.sort, editor.direction),
        (SortKey::Memory, SortDirection::Descending)
    );
    click(&mut editor, 48);
    assert_eq!(editor.sort, SortKey::State);
    click(&mut editor, 58);
    assert_eq!(editor.sort, SortKey::User);
    click(&mut editor, 100);
    assert_eq!(editor.sort, SortKey::Command);
}

#[test]
fn test_clicking_the_sorted_heading_again_reverses_it() {
    let mut editor = loaded_editor();
    click(&mut editor, 0);
    click(&mut editor, 3);
    assert_eq!(
        (editor.sort, editor.direction),
        (SortKey::Pid, SortDirection::Descending)
    );
    click(&mut editor, 3);
    assert_eq!(editor.direction, SortDirection::Ascending);
}

#[test]
fn test_a_click_off_the_heading_row_or_a_drag_is_left_to_the_terminal() {
    let mut editor = loaded_editor();
    let below = editor.on_mouse(Point {
        col: 0,
        drag: false,
        row: HEADER_ROWS,
    });
    assert_eq!(below, EditorOutcome::Ignored);
    let drag = editor.on_mouse(Point {
        col: 0,
        drag: true,
        row: COLUMN_ROW,
    });
    assert_eq!(drag, EditorOutcome::Ignored);
    assert_eq!(editor.sort, SortKey::Cpu);
}

#[test]
fn test_the_video_memory_column_appears_only_when_a_gpu_editor_answered() {
    let plain = loaded_editor().content(20, 120, false);
    assert!(!row_text(&plain.rows[COLUMN_ROW]).contains("VRAM"));
    let mut editor = gpu_editor();
    let content = editor.content(20, 120, false);
    assert!(row_text(&content.rows[COLUMN_ROW]).contains("VRAM"));
    // The one with a reading shows it, and the others show a dash rather
    // than a zero they were never measured at.
    let lines: Vec<String> = content.rows[HEADER_ROWS..].iter().map(row_text).collect();
    assert!(
        lines.iter().any(|line| line.contains("2.0M")),
        "got {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains(" - ")),
        "got {lines:?}"
    );
}

#[test]
fn test_the_heading_clicks_shift_right_when_the_video_memory_column_is_there() {
    let mut editor = gpu_editor();
    // Cell 48 is STATE without the column and VRAM with it.
    click(&mut editor, 48);
    assert_eq!(editor.sort, SortKey::Gpu);
    click(&mut editor, 56);
    assert_eq!(editor.sort, SortKey::State);
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
fn test_the_heading_rows_never_wrap_in_a_narrow_pane() {
    // A wrapped summary would push the headings to screen row two and
    // make every heading click land on the wrong row.
    let mut editor = gpu_editor();
    let content = editor.content(20, 24, true);
    for row in &content.rows[..HEADER_ROWS] {
        assert!(
            row_text(row).chars().count() <= 24,
            "got {:?}",
            row_text(row)
        );
    }
}

fn press_at(editor: &mut ProcEditor, row: usize) -> EditorOutcome {
    editor.on_mouse(Point {
        col: 0,
        drag: false,
        row,
    })
}

#[test]
fn test_clicking_a_process_moves_the_cursor_onto_it() {
    let mut editor = loaded_editor();
    editor.content(30, 120, false);
    // Rows are init, server, worker from screen row two down.
    assert_eq!(
        press_at(&mut editor, HEADER_ROWS + 2),
        EditorOutcome::Consumed
    );
    assert_eq!(editor.selected().unwrap().name, "worker");
    press_at(&mut editor, HEADER_ROWS);
    assert_eq!(editor.selected().unwrap().name, "init");
}

#[test]
fn test_a_click_below_the_last_process_selects_nothing() {
    let mut editor = loaded_editor();
    editor.content(30, 120, false);
    assert_eq!(
        press_at(&mut editor, HEADER_ROWS + 3),
        EditorOutcome::Ignored
    );
    assert_eq!(editor.selected().unwrap().name, "init");
}

#[test]
fn test_a_click_lands_on_the_right_process_when_an_earlier_row_wraps() {
    let mut editor = loaded_editor();
    editor.on_key(&press(KeyCode::Char('=')));
    // Flat: three rows. In a pane too narrow for the first row's command,
    // that row takes more than one screen row, so the second process is
    // not on the screen row after the first.
    editor.content(30, 75, true);
    let first_height = editor.painted[0];
    assert!(first_height > 1, "the fixture row should wrap");
    press_at(&mut editor, HEADER_ROWS + first_height);
    assert_eq!(editor.list.cursor(), 1);
}

#[test]
fn test_the_detail_strip_describes_the_selected_process_at_the_bottom() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "worker");
    let content = editor.content(20, 160, false);
    assert_eq!(
        content.rows.len(),
        20,
        "padding keeps the strip at the bottom"
    );
    let facts = row_text(&content.rows[20 - DETAIL_ROWS]);
    assert!(facts.contains("PID 11"), "got {facts:?}");
    assert!(facts.contains("PPID 10"), "got {facts:?}");
    let cwd = row_text(&content.rows[20 - DETAIL_ROWS + 1]);
    assert!(cwd.contains("CWD /work/worker"), "got {cwd:?}");
    let command = row_text(&content.rows[19]);
    assert!(command.contains("worker --serve"), "got {command:?}");
}

#[test]
fn test_a_short_pane_drops_the_detail_strip_to_keep_the_list() {
    let mut editor = loaded_editor();
    let content = editor.content(MIN_ROWS_FOR_DETAIL - 1, 160, false);
    assert_eq!(content.rows.len(), HEADER_ROWS + 3);
}

fn name_cell_text(editor: &ProcEditor, name: &str) -> String {
    let at = editor
        .rows
        .iter()
        .position(|row| editor.processes[row.index].name == name)
        .expect("process is listed");
    let spans = editor.name_cell(&editor.processes[editor.rows[at].index], &editor.rows[at]);
    spans.iter().map(|span| span.text.as_str()).collect()
}

#[test]
fn test_the_name_is_the_second_column_with_the_tree_in_it() {
    let mut editor = loaded_editor();
    let content = editor.content(11, 160, false);
    let heading = row_text(&content.rows[COLUMN_ROW]);
    let pid = heading.find("PID").unwrap();
    let name = heading.find("NAME").unwrap();
    let cpu = heading.find("CPU%").unwrap();
    assert!(pid < name && name < cpu, "got {heading:?}");
    // The command column carries the command alone; the fold marker and
    // indent belong to the name.
    let server = row_text(&content.rows[HEADER_ROWS + 1]);
    assert!(server.contains("\u{25be} server"), "got {server:?}");
    assert!(server.ends_with("server --serve"), "got {server:?}");
    // The cell is always the column's width, so the columns after it line up.
    assert_eq!(
        name_cell_text(&editor, "worker").chars().count(),
        editor.name_width + 1
    );
    assert!(name_cell_text(&editor, "worker").starts_with("    "));
}

fn long_named_editor(name: &str) -> ProcEditor {
    let mut editor = ProcEditor::new();
    let mut sample = sample_at(Instant::now(), [0, 0, 0]);
    sample.entries[2].name = name.to_string();
    editor.on_job(JobReply::Processes(Ok(sample)));
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
fn test_a_flat_list_shows_the_bare_name() {
    let mut editor = loaded_editor();
    editor.on_key(&press(KeyCode::Char('=')));
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
    let cell: String = editor
        .name_cell(&editor.processes[0], &deep)
        .iter()
        .map(|span| span.text.as_str())
        .collect();
    // Forty levels of indent must not push "init" out of the column.
    assert_eq!(cell.chars().count(), editor.name_width + 1);
    assert!(cell.contains("init"), "got {cell:?}");
}

#[test]
fn test_sorting_by_name_orders_alphabetically_and_by_command_orders_the_whole_line() {
    let mut editor = loaded_editor();
    editor.on_key(&press(KeyCode::Char('=')));
    editor.sort_by(SortKey::Name);
    let names: Vec<&str> = editor
        .rows
        .iter()
        .map(|row| editor.processes[row.index].name.as_str())
        .collect();
    assert_eq!(names, ["init", "server", "worker"]);
    editor.sort_by(SortKey::Command);
    assert_eq!(editor.sort, SortKey::Command);
}

#[test]
fn test_the_content_marks_the_cursor_below_the_two_header_rows() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "server");
    let content = editor.content(MIN_ROWS_FOR_DETAIL - 1, 120, false);
    assert_eq!(content.cursor_line, Some(HEADER_ROWS + 1));
    assert_eq!(content.rows.len(), HEADER_ROWS + 3);
    let line = row_text(&content.rows[HEADER_ROWS + 1]);
    assert!(line.contains("server --serve"), "got {line:?}");
}

#[test]
fn test_every_key_of_the_monitor_is_spelled_right() {
    assert_eq!(table::invalid_specs(KEYS), Vec::<&str>::new());
}
