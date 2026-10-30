//! The Docker view's tests.

use super::*;

use crate::model::input::KeyCode;
use crate::model::paint::row_text;

fn press(code: KeyCode) -> Key {
    Key {
        alt: false,
        code,
        ctrl: false,
        shift: false,
        sup: false,
    }
}

fn press_char(c: char) -> Key {
    press(KeyCode::Char(c))
}

fn reply(tag: &'static str, code: i32, stdout: &str) -> JobReply {
    JobReply::Command(CommandOutput {
        id: 0,
        code: Some(code),
        stderr: String::new(),
        stdout: stdout.to_string(),
        tag,
    })
}

fn command_of(outcome: &EditorOutcome) -> CommandRequest {
    match outcome {
        EditorOutcome::Job(JobRequest::Command(command)) => command.clone(),
        other => panic!("expected a command, got {other:?}"),
    }
}

const CONTAINERS: &str = "abc\tweb\tnginx\trunning\tUp 2 hours\t80/tcp\tshop\n\
                          def\tdb\tpostgres\tpaused\tUp 1 day (Paused)\t\tshop\n";

/// A view that has listed its containers.
fn loaded() -> DockerEditor {
    let mut editor = DockerEditor::new();
    let first = editor.on_tick(Instant::now());
    assert_eq!(command_of(&first).tag, Section::Containers.list_tag());
    editor.on_job(reply(Section::Containers.list_tag(), 0, CONTAINERS));
    editor
}

fn text_of(editor: &mut DockerEditor) -> Vec<String> {
    editor
        .content(20, 120, false)
        .rows
        .iter()
        .map(row_text)
        .collect()
}

#[test]
fn test_every_key_of_the_view_is_spelled_right() {
    for keys in [KEYS, CONTAINER_KEYS, IMAGE_KEYS, OUTPUT_KEYS] {
        assert_eq!(table::invalid_specs(keys), Vec::<&str>::new());
    }
}

#[test]
fn test_every_menu_entry_is_a_key_the_view_answers() {
    for section in Section::ALL {
        let mut editor = loaded();
        editor.section = section;
        let groups: &[&[Act]] = match section {
            Section::Containers => &CONTAINER_MENU,
            Section::Images => &IMAGE_MENU,
            Section::Networks | Section::Volumes => &OBJECT_MENU,
        };
        for act in groups.iter().flat_map(|group| group.iter()) {
            let bound = table::menu_entry(KEYS, *act)
                .or_else(|| table::menu_entry(editor.section_keys(), *act));
            assert!(bound.is_some(), "{act:?} in {section:?}");
        }
    }
}

#[test]
fn test_the_containers_are_listed_with_their_state() {
    let mut editor = loaded();
    let text = text_of(&mut editor);
    assert!(text[0].contains("Containers") && text[0].contains("2 listed"));
    assert!(text[1].starts_with("NAME"));
    assert!(text[2].starts_with("web") && text[2].contains("running"));
}

#[test]
fn test_a_key_acts_on_the_container_under_the_cursor_and_lists_again_soon() {
    let mut editor = loaded();
    let stop = command_of(&editor.on_key(&press_char('x')));
    assert_eq!(stop.program, "docker");
    assert_eq!(stop.args, ["stop", "web"]);
    editor.on_key(&press_char('j'));
    let unpause = command_of(&editor.on_key(&press_char('p')));
    assert_eq!(unpause.args, ["unpause", "db"], "a paused container is unpaused");
    editor.on_job(reply(TAG_ACT, 0, ""));
    assert_eq!(editor.take_notice().map(|n| n.text), Some("unpause db".to_string()));
}

#[test]
fn test_removing_asks_first_and_only_a_yes_removes() {
    let mut editor = loaded();
    let EditorOutcome::Prompt(question) = editor.on_key(&press_char('D')) else {
        panic!("a question");
    };
    assert!(question.label.contains("container web"));
    let declined = editor.on_prompt(PromptReply {
        answer: None,
        tag: ASK_CONFIRM,
    });
    assert_eq!(declined, EditorOutcome::Consumed);
    editor.on_key(&press_char('D'));
    let removed = editor.on_prompt(PromptReply {
        answer: Some("y".to_string()),
        tag: ASK_CONFIRM,
    });
    assert_eq!(command_of(&removed).args, ["container", "rm", "web"]);
}

#[test]
fn test_a_shell_opens_in_a_terminal_tab() {
    let mut editor = loaded();
    let EditorOutcome::Spawn(spawn) = editor.on_key(&press_char('e')) else {
        panic!("a terminal tab");
    };
    assert_eq!(spawn.program, "docker");
    assert_eq!(spawn.args[..3], ["exec", "-it", "web"].map(String::from));
}

#[test]
fn test_tab_moves_to_the_images_and_a_late_container_listing_is_dropped() {
    let mut editor = loaded();
    let listing = command_of(&editor.on_key(&press(KeyCode::Tab)));
    assert_eq!(listing.args[0], "images");
    editor.on_job(reply(Section::Containers.list_tag(), 0, CONTAINERS));
    assert!(editor.items.is_empty(), "an answer for the containers is not shown here");
    editor.on_job(reply(Section::Images.list_tag(), 0, "ff43\tapp\tfixed\t212MB\t6 hours ago\n"));
    let text = text_of(&mut editor);
    assert!(text[2].starts_with("app:fixed"));
    assert!(editor.keys().iter().any(|help| help.label == "Pull it again"));
    assert!(!editor.keys().iter().any(|help| help.label == "Stop"));
}

#[test]
fn test_inspect_fills_the_view_until_q_goes_back() {
    let mut editor = loaded();
    let read = command_of(&editor.on_key(&press(KeyCode::Enter)));
    assert_eq!(read.args, ["container", "inspect", "web"]);
    editor.on_job(reply(TAG_READ, 0, "[\n  {\"Id\": \"abc\"}\n]\n"));
    let text = text_of(&mut editor);
    assert!(text[0].contains("Inspect: web"));
    assert_eq!(text[2].trim(), "{\"Id\": \"abc\"}");
    assert_eq!(editor.next_tick(), None, "no listing under output");
    let back = editor.on_key(&press_char('q'));
    assert_eq!(command_of(&back).tag, Section::Containers.list_tag());
    assert!(editor.output.is_none());
}

#[test]
fn test_a_daemon_that_cannot_be_reached_is_said_in_the_view() {
    let mut editor = DockerEditor::new();
    editor.on_tick(Instant::now());
    editor.on_job(JobReply::Command(CommandOutput {
        id: 0,
        code: Some(1),
        stderr: "Cannot connect to the Docker daemon".to_string(),
        stdout: String::new(),
        tag: Section::Containers.list_tag(),
    }));
    assert!(text_of(&mut editor)
        .iter()
        .any(|row| row.contains("Cannot connect")));
}
