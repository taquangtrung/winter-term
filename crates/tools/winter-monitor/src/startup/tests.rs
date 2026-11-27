//! The startup monitor's tests.

use misty::testing::{TestWindow, drawn_text};
use winter_model::editor::CommandOutput;
use winter_model::input::KeyCode;

use super::*;

fn press(code: KeyCode) -> Key {
    Key::plain(code)
}

fn entry(kind: StartupKind, scope: StartupScope, name: &str, enabled: bool) -> StartupEntry {
    StartupEntry {
        command: format!("/usr/bin/{name}"),
        enabled,
        kind,
        name: name.to_string(),
        scope,
        source: Some(PathBuf::from(format!("/etc/{name}"))),
    }
}

fn loaded_editor() -> StartupEditor {
    let mut editor = StartupEditor::new();
    editor.on_job(JobReply::Monitor(MonitorAnswer::Startup(Ok(vec![
        entry(
            StartupKind::Systemd,
            StartupScope::System,
            "sshd.service",
            true,
        ),
        entry(
            StartupKind::Systemd,
            StartupScope::User,
            "syncthing.service",
            false,
        ),
        entry(
            StartupKind::Autostart,
            StartupScope::User,
            "Nextcloud",
            true,
        ),
    ]))));
    editor
}

fn drawn(editor: &mut StartupEditor, window: &TestWindow) -> misty::Frame {
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

/// Put the cursor on the entry named `name`.
fn cursor_on(editor: &mut StartupEditor, name: &str) {
    let at = editor
        .rows
        .iter()
        .position(|at| editor.entries[*at].name == name)
        .expect("listed");
    editor.list.set_cursor(at);
}

#[test]
fn test_it_reads_when_first_shown_and_never_on_a_timer() {
    let mut editor = StartupEditor::new();
    assert_eq!(editor.next_tick(), None, "nothing read before it is shown");
    assert_eq!(
        editor.set_shown(true),
        EditorOutcome::Job(JobRequest::Monitor(MonitorQuery::Startup))
    );
    assert_eq!(
        editor.set_shown(true),
        EditorOutcome::Consumed,
        "one reading at a time"
    );
    editor.on_job(JobReply::Monitor(MonitorAnswer::Startup(Ok(Vec::new()))));
    editor.set_shown(false);
    assert_eq!(
        editor.set_shown(true),
        EditorOutcome::Consumed,
        "read once, not each showing"
    );
    assert_eq!(editor.next_tick(), None);
}

#[test]
fn test_entries_are_grouped_by_kind_and_show_whether_they_are_on() {
    let mut editor = loaded_editor();
    let names: Vec<&str> = editor
        .rows
        .iter()
        .map(|at| editor.entries[*at].name.as_str())
        .collect();
    assert_eq!(names, ["Nextcloud", "sshd.service", "syncthing.service"]);
    let text = drawn_text(&drawn(&mut editor, &TestWindow::new(1400, 700))).join("\n");
    assert!(text.contains("3 entries, 2 on"), "{text:?}");
    assert!(text.contains("off"), "{text:?}");
}

#[test]
fn test_t_turns_a_user_service_on_and_reads_the_list_again() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "syncthing.service");
    let EditorOutcome::Job(JobRequest::Command(request)) =
        editor.on_key(&press(KeyCode::Char('t')))
    else {
        panic!("a user's own service is turned on in place");
    };
    assert_eq!(request.args, ["--user", "enable", "syncthing.service"]);
    let done = editor.on_job(JobReply::Command(CommandOutput {
        code: Some(0),
        tag: TAG_TOGGLE,
        ..CommandOutput::default()
    }));
    assert_eq!(
        done,
        EditorOutcome::Job(JobRequest::Monitor(MonitorQuery::Startup))
    );
}

#[test]
fn test_t_on_what_it_cannot_change_says_how_instead() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "sshd.service");
    assert_eq!(
        editor.on_key(&press(KeyCode::Char('t'))),
        EditorOutcome::Consumed
    );
    let notice = editor.take_notice().expect("it says why");
    assert!(
        format!("{notice:?}").contains("sudo systemctl disable sshd.service"),
        "{notice:?}"
    );
}

#[test]
fn test_enter_opens_the_file_that_sets_an_entry_up() {
    let mut editor = loaded_editor();
    cursor_on(&mut editor, "Nextcloud");
    assert_eq!(
        editor.on_key(&press(KeyCode::Enter)),
        EditorOutcome::OpenPath(OpenTarget {
            column: None,
            line: None,
            path: PathBuf::from("/etc/Nextcloud"),
        })
    );
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
