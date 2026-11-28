//! The ports monitor's tests.

use misty::testing::{TestWindow, drawn_text};
use winter_model::input::KeyCode;

use super::*;

fn press(code: KeyCode) -> Key {
    Key::plain(code)
}

fn socket(port: u16, listening: bool, pid: Option<u32>, program: &str) -> Socket {
    Socket {
        listening,
        local: format!("127.0.0.1:{port}"),
        pid,
        port,
        program: pid.map(|_| program.to_string()),
        protocol: Protocol::Tcp,
        remote: (!listening).then(|| "10.0.0.2:50000".to_string()),
        state: match listening {
            true => "LISTEN".to_string(),
            false => "ESTABLISHED".to_string(),
        },
    }
}

fn loaded_editor() -> PortsEditor {
    let mut editor = PortsEditor::new();
    editor.on_job(JobReply::Monitor(MonitorAnswer::Ports(Ok(vec![
        socket(8080, true, Some(40), "python"),
        socket(3000, true, Some(812), "node"),
        socket(3000, false, Some(812), "node"),
        socket(22, true, None, ""),
    ]))));
    editor
}

/// `editor` drawn over `window`, told the window's text rows and columns
/// first as the window tells a page.
fn drawn(editor: &mut PortsEditor, window: &TestWindow) -> misty::Frame {
    let mut frame = window.frame();
    let bounds = frame.bounds();
    let cell = frame.cell_metrics();
    editor.content(
        (bounds.height / cell.height) as usize,
        (bounds.width / cell.width) as usize,
        false,
    );
    editor.draw_view(&mut frame, bounds, &misty::Theme::dark());
    frame
}

fn text_of(editor: &mut PortsEditor) -> String {
    drawn_text(&drawn(editor, &TestWindow::new(1400, 700))).join("\n")
}

#[test]
fn test_the_first_tick_asks_for_the_sockets_and_none_comes_while_hidden() {
    let mut editor = PortsEditor::new();
    assert_eq!(
        editor.on_tick(Instant::now()),
        EditorOutcome::Job(JobRequest::Monitor(MonitorQuery::Ports))
    );
    let mut hidden = PortsEditor::new();
    hidden.set_shown(false);
    assert_eq!(hidden.next_tick(), None, "a page not shown reads nothing");
}

#[test]
fn test_listening_sockets_are_listed_by_port_and_a_toggles_every_socket() {
    let mut editor = loaded_editor();
    let ports: Vec<u16> = editor
        .rows
        .iter()
        .map(|at| editor.sockets[*at].port)
        .collect();
    assert_eq!(ports, [22, 3000, 8080], "the connection is left out");
    editor.on_key(&press(KeyCode::Char('a')));
    assert_eq!(editor.rows.len(), 4);
    let text = text_of(&mut editor);
    assert!(text.contains("10.0.0.2:50000"), "{text:?}");
}

#[test]
fn test_the_cursor_stays_on_its_socket_when_a_refresh_reorders_them() {
    let mut editor = loaded_editor();
    editor.on_key(&press(KeyCode::Char('j')));
    assert_eq!(editor.selected().unwrap().port, 3000);
    editor.on_job(JobReply::Monitor(MonitorAnswer::Ports(Ok(vec![
        socket(3000, true, Some(812), "node"),
        socket(1, true, Some(2), "first"),
        socket(22, true, None, ""),
    ]))));
    assert_eq!(editor.selected().unwrap().port, 3000);
}

#[test]
fn test_x_asks_before_ending_the_holder_and_a_socket_with_none_says_so() {
    let mut editor = loaded_editor();
    // Port 22's holder is another user's: there is no one to end.
    assert_eq!(
        editor.on_key(&press(KeyCode::Char('x'))),
        EditorOutcome::Consumed
    );
    assert!(matches!(editor.notice, Some(ProcNotice::Failed(_))));
    editor.on_key(&press(KeyCode::Char('j')));
    let EditorOutcome::Confirm(question) = editor.on_key(&press(KeyCode::Char('x'))) else {
        panic!("ending a process asks first");
    };
    assert!(
        question.message().contains("node (pid 812)"),
        "{:?}",
        question.message()
    );
    let outcome = editor.on_confirm(true);
    let EditorOutcome::Job(JobRequest::Command(request)) = outcome else {
        panic!("said yes to, the signal is sent: {outcome:?}");
    };
    assert!(
        request.args.contains(&"812".to_string()),
        "{:?}",
        request.args
    );
}

#[test]
fn test_a_filter_narrows_the_list_and_escape_lets_go_of_it() {
    let mut editor = loaded_editor();
    editor.on_prompt_input("python", ASK_FILTER);
    assert_eq!(editor.rows.len(), 1);
    editor.on_key(&press(KeyCode::Escape));
    assert_eq!(editor.rows.len(), 3);
    assert_eq!(
        editor.on_key(&press(KeyCode::Escape)),
        EditorOutcome::Ignored,
        "with no filter, Escape is left to close the tab"
    );
}

#[test]
fn test_a_heading_click_sorts_by_its_column_and_a_second_turns_it_round() {
    let mut editor = loaded_editor();
    let first = |editor: &PortsEditor| editor.sockets[editor.rows[0]].program.clone();
    editor.sort_on(SortKey::Program);
    assert_eq!(first(&editor), None, "a holder no one names sorts first");
    editor.sort_on(SortKey::Program);
    assert_eq!(first(&editor).as_deref(), Some("python"));
}

#[test]
fn test_every_key_of_the_monitor_is_spelled_right() {
    assert_eq!(table::invalid_specs(&KEYS), Vec::<&str>::new());
}

#[test]
fn test_every_control_says_what_it_is_and_lights_under_the_pointer() {
    let mut editor = loaded_editor();
    let mut window = TestWindow::new(1400, 700);
    let frame = drawn(&mut editor, &window);
    let unnamed = misty::testing::unnamed_controls(&frame);
    assert!(unnamed.is_empty(), "{unnamed:?}");
    let mut state = (&mut editor, &mut window);
    let unlit = misty::testing::unlit_controls(
        &mut state,
        |(editor, window), input| {
            editor.view_input(input, &mut window.cx());
        },
        |(editor, window)| drawn(editor, window),
    );
    assert!(unlit.is_empty(), "{unlit:?}");
}
